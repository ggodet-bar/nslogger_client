use std::io ;
use std::io::Write ;
use std::thread ;
use std::thread::Thread ;
use std::sync::mpsc ;
use std::sync::atomic::{AtomicU32, Ordering} ;
use std::net::TcpStream ;
use std::collections::HashMap ;
use std::fs::File ;
use std::io::BufWriter ;
use std::path::PathBuf ;

use openssl ;
use openssl::ssl::{SslMethod, SslConnectorBuilder, SslStream} ;

use nslogger::log_message::{LogMessage, LogMessageType, MessagePartKey} ;

use nslogger::DEBUG_LOGGER ;
use nslogger::LoggerOptions ;
use nslogger::{USE_SSL, BROWSE_BONJOUR} ;

#[derive(Debug)]
pub enum HandlerMessageType {
    TRY_CONNECT,
    CONNECT_COMPLETE,
    ADD_LOG(LogMessage),
    ADD_LOG_RECORD,
    OPTION_CHANGE(HashMap<String, String>),
    QUIT
}

#[derive(Debug)]
pub enum WriteStreamWrapper {
    Tcp(TcpStream),
    Ssl(SslStream<TcpStream>),
    File(BufWriter<File>)
}

impl WriteStreamWrapper {
    pub fn write_all(&mut self, buf:&[u8]) -> io::Result<()> {
        self.unwrap().write_all(buf)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.unwrap().flush()
    }

    fn unwrap(&mut self) -> &mut Write {
        match *self {
            WriteStreamWrapper::Tcp(ref mut stream) => stream,
            WriteStreamWrapper::Ssl(ref mut stream) => stream,
            WriteStreamWrapper::File(ref mut stream) => stream,
        }
    }
}


pub struct LoggerState
{
    pub ready:bool,
    pub ready_waiters: Vec<Thread>,
    pub options:LoggerOptions,
    pub is_reconnection_scheduled: bool,
    pub is_connecting: bool,
    pub is_connected: bool,
    pub is_handler_running: bool,
    pub is_client_info_added: bool,
    pub bonjour_service_type: Option<String>,
    pub bonjour_service_name: Option<String>,
    /// the remote host we're talking to
    pub remote_host:Option<String>,
    pub remote_port:Option<u16>,

    pub write_stream:Option<WriteStreamWrapper>,

    /// file or socket output stream
    //pub write_stream:Option<Write + 'static:std::marker::Sized>,

    next_sequence_numbers:AtomicU32,
    pub log_messages:Vec<LogMessage>,
    message_sender:mpsc::Sender<HandlerMessageType>,
    pub message_receiver:Option<mpsc::Receiver<HandlerMessageType>>,

    pub log_file_path:Option<PathBuf>,
}

impl LoggerState
{
    pub fn new(message_sender:mpsc::Sender<HandlerMessageType>, message_receiver:mpsc::Receiver<HandlerMessageType>) -> LoggerState {
        LoggerState{  options: BROWSE_BONJOUR | USE_SSL,
                      ready_waiters: vec![],
                      bonjour_service_type: None,
                      bonjour_service_name: None,
                      remote_host: None,
                      remote_port: None,
                      write_stream: None,
                      is_reconnection_scheduled: false,
                      is_connecting: false,
                      is_connected: false,
                      is_handler_running: false,
                      ready: false,
                      is_client_info_added: false,
                      next_sequence_numbers: AtomicU32::new(0),
                      log_messages: vec![],
                      message_sender: message_sender,
                      message_receiver: Some(message_receiver),
                      log_file_path: None,
        }
    }

    pub fn process_log_queue(&mut self) {
        if self.log_messages.is_empty() {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "process_log_queue empty") ;
            }
            return ;
        }

        if !self.is_client_info_added
        {
            self.push_client_info_to_front_of_queue() ;
        }


        // FIXME TONS OF STUFF SKIPPED!!

        if self.remote_host.is_none() {
            self.flush_queue_to_buffer_stream() ;
        }
        else if self.write_stream.is_none() {
            // the host is set by the socket isn't opened yet
            self.disconnect_from_remote() ;
            self.try_reconnecting() ;
        }
        else if self.is_connected {
            // FIXME SKIPPING SOME OTHER STUFF

            self.write_messages_to_stream() ;
        }

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "[{:?}] finished processing log queue", thread::current().id()) ;
        }
    }

    fn push_client_info_to_front_of_queue(&mut self) {
        use sys_info ;
        use std::env ;
        use std::ffi::OsStr ;
        use std::path::Path ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "pushing client info to front of queue") ;
        }

        let mut message = LogMessage::new(LogMessageType::CLIENT_INFO, self.get_and_increment_sequence_number()) ;

        match sys_info::os_type() {
            Ok(name) => {
                message.add_string(MessagePartKey::OS_NAME, &name) ;

                match sys_info::os_release() {
                    Ok(release) => message.add_string(MessagePartKey::OS_VERSION, &release),
                    _ => ()
                } ;
            },
            _ => ()
        } ;

        let process_name = env::current_exe().ok()
                                            .as_ref()
                                            .map(Path::new)
                                            .and_then(Path::file_name)
                                            .and_then(OsStr::to_str)
                                            .map(String::from) ;

        if process_name.is_some() {
            message.add_string(MessagePartKey::CLIENT_NAME, &process_name.unwrap()) ;
        }


        self.log_messages.insert(0, message) ;
        self.is_client_info_added = true ;
    }

    pub fn change_options(&mut self, new_options:HashMap<String, String>) {
        match new_options.get("filename") {
            Some(filename) => if self.log_file_path.is_none()
                                    || self.log_file_path.as_ref().unwrap() != &PathBuf::from(filename) {
                                        if self.log_file_path.is_none() {
                                            self.disconnect_from_remote() ;
                                        }
                                        else if self.write_stream.is_some() {
                                            self.close_buffer_write_stream() ;
                                        }

                                        self.log_file_path = Some(PathBuf::from(filename)) ;
                                        self.create_buffer_write_stream() ;
            },
            None => {
                let mut change = self.log_file_path.is_some() ;

                let mut new_flags = LoggerOptions::empty() ;
                let mut host:Option<String> = None ;
                let mut port:Option<u16> = None ;

                match new_options.get("use_ssl") {
                    Some(ssl) => if ssl == "1" {
                        new_flags |= USE_SSL ;
                    },
                    None => ()
                } ;

                match new_options.get("bonjour_server") {
                    Some(_) => new_flags |= BROWSE_BONJOUR,
                    None => {
                        host = new_options.get("remote_host").cloned() ;
                        port = new_options.get("remote_port")
                                          .and_then(| port_string | {
                                            u16::from_str_radix(port_string, 10).ok()
                                          }) ;

                        if !change && (self.options & BROWSE_BONJOUR).is_empty() {
                            // check if the port or host is changing
                            change = self.remote_host.is_none()
                                        || self.remote_host.as_ref().unwrap() != host.as_ref().unwrap()
                                        || self.remote_port.as_ref().unwrap() != port.as_ref().unwrap() ;


                        }
                    }
                } ;

                if new_flags != self.options || change {
                    if DEBUG_LOGGER {
                        info!(target:"NSLogger", "changing options: {:?}. Closing/restarting.", new_options) ;
                    }

                    if self.log_file_path.is_some() {
                        self.close_buffer_write_stream() ;
                    }
                    else {
                        self.disconnect_from_remote() ;
                    }

                    self.options = new_flags ;

                    if (new_flags & BROWSE_BONJOUR).is_empty() {
                        self.remote_host = host ;
                        self.remote_port = port ;

                        self.connect_to_remote() ;
                    }
                    else {
                        self.setup_bonjour() ;
                    }
                }
            }
        } ;
        self.connect_to_remote() ;
    }

    pub fn setup_bonjour(&mut self) {
        use tokio_core::reactor::{Core,Timeout} ;
        use futures::future::Either ;
        use async_dnssd ;
        use async_dnssd::Interface ;
        use std::net::ToSocketAddrs ;
        use std::time::Duration ;
        use futures::{Stream,Future} ;

        if (self.options & BROWSE_BONJOUR).is_empty() {
            self.close_bonjour() ;
        }
        else {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "Setting up Bonjour") ;
            }

            let service_type = if (self.options & USE_SSL).is_empty() {
                "_nslogger._tcp"
            } else {
                "_nslogger-ssl._tcp"
            } ;

            self.bonjour_service_type = Some(service_type.to_string()) ;
            let mut core = Core::new().unwrap() ;
            let handle = core.handle() ;

            let mut listener = async_dnssd::browse(Interface::Any, service_type, None, &handle).unwrap() ;

            let timeout = Timeout::new(Duration::from_secs(5), &handle).unwrap() ;
            match core.run(listener.into_future().select2(timeout)) {
                Ok( either ) => {
                    match either {
                       Either::A(( ( result, _ ), _ )) => {
                           let browse_result = result.unwrap() ;
                           if DEBUG_LOGGER {
                                info!(target:"NSLogger", "Browse result: {:?}", browse_result) ;
                                info!(target:"NSLogger", "Service name: {}", browse_result.service_name) ;
                           }
                            self.bonjour_service_name = Some(browse_result.service_name.to_string()) ;
                            match core.run(browse_result.resolve(&handle).unwrap().into_future()) {
                                Ok( (resolve_result, resolve) ) => {
                                    let resolve_details = resolve_result.unwrap() ;
                                    if DEBUG_LOGGER {
                                        info!(target:"NSLogger", "Service resolution details: {:?}", resolve_details) ;
                                    }
                                    for host_addr in format!("{}:{}", resolve_details.host_target, resolve_details.port).to_socket_addrs().unwrap() {


                                        if !host_addr.ip().is_global() && host_addr.ip().is_ipv4() {
                                            let ip_address = format!("{}", host_addr.ip()) ;
                                            if DEBUG_LOGGER {
                                                info!(target:"NSLogger", "Bonjour host details {:?}", host_addr) ;
                                            }
                                            self.remote_host = Some(ip_address) ;
                                            self.remote_port = Some(resolve_details.port) ;
                                            break ;
                                        }

                                    }

                                    self.message_sender.send(HandlerMessageType::TRY_CONNECT) ;
                                },
                                Err(b) => {
                                    if DEBUG_LOGGER {
                                        warn!(target:"NSLogger", "Couldn't resolve Bonjour service")
                                    }
                                }
                            } ;
                        },
                        Either::B( ( timeout, browse ) ) => {
                            if DEBUG_LOGGER {
                                warn!(target:"NSLogger", "Bonjour discovery timed out")
                            }
                        }
                    }
                },
                Err(b) => if DEBUG_LOGGER {
                    warn!(target:"NSLogger", "Couldn't resolve Bonjour service")
                }

            } ;
        }
    }


    pub fn close_bonjour(&mut self) {
        // FIXME FILL THAT
    }

    pub fn connect_to_remote(&mut self) -> Result<(), &str> {
        //if self.write_stream.is_some() {
            //return Err("internal error: write_stream should be none") ;
        //}
        if self.write_stream.is_some() {
            return Err("internal error: remote_socket should be none") ;
        }

        //close_bonjour() ;

        let remote_host = self.remote_host.as_ref().unwrap() ;
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "connecting to {}:{}", remote_host, self.remote_port.unwrap()) ;
        }

        let connect_string = format!("{}:{}", remote_host, self.remote_port.unwrap()) ;
        let stream = match TcpStream::connect(connect_string) {
            Ok(s) => s,
            Err(e) => return Err("error occurred during tcp stream connection")
        } ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "{:?}", &stream) ;
        }
        self.write_stream = Some(WriteStreamWrapper::Tcp(stream)) ;
        if !(self.options | USE_SSL).is_empty() {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "activating SSL connection") ;
            }

            let mut ssl_connector_builder = SslConnectorBuilder::new(SslMethod::tls()).unwrap() ;

            ssl_connector_builder.builder_mut().set_verify(openssl::ssl::SSL_VERIFY_NONE) ;
            ssl_connector_builder.builder_mut().set_verify_callback(openssl::ssl::SSL_VERIFY_NONE, |_,_| { true }) ;

            let connector = ssl_connector_builder.build() ;
            if let WriteStreamWrapper::Tcp(inner_stream) = self.write_stream.take().unwrap() {
                let stream = connector.danger_connect_without_providing_domain_for_certificate_verification_and_server_name_indication(inner_stream).unwrap();
                self.write_stream = Some(WriteStreamWrapper::Ssl(stream)) ;
            }

            self.message_sender.send(HandlerMessageType::CONNECT_COMPLETE) ;

        }
        else {
            self.message_sender.send(HandlerMessageType::CONNECT_COMPLETE) ;
        }

        //remoteSocket = new Socket(remoteHost, remotePort);
        //if ((options & OPT_USE_SSL) != 0)
        //{
            //if (DEBUG_LOGGER)
                //Log.v("NSLogger", "activating SSL connection");

            //SSLSocketFactory sf = SSLCertificateSocketFactory.getInsecure(5000, null);
            //remoteSocket = sf.createSocket(remoteSocket, remoteHost, remotePort, true);
            //if (remoteSocket != null)
            //{
                //if (DEBUG_LOGGER)
                    //Log.v("NSLogger", String.format("starting SSL handshake with %s:%d", remoteSocket.getInetAddress().toString(), remoteSocket.getPort()));

                //SSLSocket socket = (SSLSocket) remoteSocket;
                //socket.setUseClientMode(true);
                //writeStream = remoteSocket.getOutputStream();
                //socketSendBufferSize = remoteSocket.getSendBufferSize();
                //loggingThreadHandler.sendMessage(loggingThreadHandler.obtainMessage(MSG_CONNECT_COMPLETE));
            //}
        //}
        //else
        //{
            //// non-SSL sockets are immediately ready for use
            //socketSendBufferSize = remoteSocket.getSendBufferSize();
            //writeStream = remoteSocket.getOutputStream();
            //loggingThreadHandler.sendMessage(loggingThreadHandler.obtainMessage(MSG_CONNECT_COMPLETE));
        //}
        Ok( () )
    }

    fn disconnect_from_remote(&mut self) {
        self.is_connected = false ;

        if self.write_stream.is_none() {
            return ;
        }


        if DEBUG_LOGGER {
            info!(target:"NSLogger", "disconnect_from_remote()") ;
        }

        self.write_stream.take() ;

        self.is_connecting = false ;
        self.is_client_info_added = false ;
    }

    fn try_reconnecting(&mut self) {
        // TODO
    }

    pub fn create_buffer_write_stream(&mut self) {
        use std::fs::File ;
        use std::io::BufWriter ;
        use nslogger::logger_state::WriteStreamWrapper ;

        if self.log_file_path.is_none() {
            return ;
        }

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "Creating file buffer stream to {:?}", self.log_file_path.as_ref().unwrap()) ;
        }

        let file_writer = BufWriter::new(File::create(self.log_file_path.as_ref().unwrap()).unwrap()) ;
        self.write_stream = Some(WriteStreamWrapper::File(file_writer)) ;
        self.flush_queue_to_buffer_stream() ;
    }


    pub fn close_buffer_write_stream(&mut self) {
        use std::io::Write ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "Closing file buffer stream") ;
        }

        let mut file_stream = self.write_stream.take().unwrap() ;

        file_stream.flush() ;

        // Letting the file go out of scope will close it
    }

    pub fn get_and_increment_sequence_number(&mut self) -> u32 {
        return self.next_sequence_numbers.fetch_add(1, Ordering::SeqCst) ;
    }


    /// Write outstanding messages to the buffer file
    pub fn flush_queue_to_buffer_stream(&mut self) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "flush_queue_to_buffer_stream") ;
        }

        self.write_messages_to_stream() ;
    }

    fn write_messages_to_stream(&mut self) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "process_log_queue: {} queued messages", self.log_messages.len()) ;
        }

        while !self.log_messages.is_empty() {
            {
                let message = self.log_messages.first().unwrap() ;
                if DEBUG_LOGGER {
                    info!(target:"NSLogger", "processing message {}", &message.sequence_number) ;
                }

                let message_vec = message.get_bytes() ;
                let message_bytes = message_vec.as_slice() ;
                let length = message_bytes.len() ;
                if DEBUG_LOGGER {
                    use std::cmp ;
                    if DEBUG_LOGGER {
                        info!(target:"NSLogger", "length: {}", length) ;
                        info!(target:"NSLogger", "bytes: {:?}", &message_bytes[0..cmp::min(length, 40)]) ;
                    }
                }

                {
                    let mut tcp_stream = self.write_stream.as_mut().unwrap() ;
                    tcp_stream.write_all(message_bytes).expect("Write to stream failed") ;
                }

                match message.flush_rx {
                    None => message.flush_tx.send(true).unwrap(),
                    _ => ()
                }
            }


            self.log_messages.remove(0) ;
        }
    }
}
