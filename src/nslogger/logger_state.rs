use std::io ;
use std::io::Write ;
use std::thread ;
use std::thread::Thread ;
use std::sync::mpsc ;
use std::sync::atomic::Ordering ;
use integer_atomics::AtomicU32 ;
use std::net::TcpStream ;
use std::collections::HashMap ;
use std::fs::File ;
use std::io::BufWriter ;
use std::path::PathBuf ;

use openssl ;
use openssl::ssl::{SslMethod, SslConnector, SslStream} ;

use nslogger::log_message::{LogMessage, LogMessageType, MessagePartKey} ;
use nslogger::network_manager ;

use nslogger::DEBUG_LOGGER ;
use nslogger::LoggerOptions ;
use nslogger::{USE_SSL, BROWSE_BONJOUR} ;

#[derive(Debug)]
pub enum HandlerMessageType {
    TryConnect,
    TryConnectBonjour(String, String, u16),
    ConnectComplete,
    AddLog(LogMessage),
    OptionChange(HashMap<String, String>),
    Quit
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

    action_sender:mpsc::Sender<network_manager::NetworkActionMessage>,
    action_receiver:Option<mpsc::Receiver<network_manager::NetworkActionMessage>>,
}

impl LoggerState
{
    pub fn new(message_sender:mpsc::Sender<HandlerMessageType>, message_receiver:mpsc::Receiver<HandlerMessageType>) -> LoggerState {
        let (action_sender, action_receiver) = mpsc::channel() ;

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

                      action_sender: action_sender,
                      action_receiver: Some(action_receiver),
        }
    }

    pub fn process_log_queue(&mut self) {
        if self.log_messages.is_empty() {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "process_log_queue empty") ;
            }
            return ;
        }

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "process_log_queue") ;
        }

        self.setup_network_manager_if_required() ;

        if !self.is_client_info_added
        {
            self.push_client_info_to_front_of_queue() ;
        }

        if self.write_stream.is_none() {
            if self.log_file_path.is_some() {
                self.create_buffer_write_stream() ;
            }
            else if !(self.is_connecting
                      || self.is_reconnection_scheduled
                      || !(self.options & BROWSE_BONJOUR).is_empty() ) {

                if self.remote_host.is_some() && self.remote_port.is_some() {
                    self.connect_to_remote() ;
                }
                else {
                    self.setup_bonjour() ;
                }
            }

            return ;
        }

        if self.remote_host.is_none() {
            self.flush_queue_to_buffer_stream() ;
        }
        else if self.write_stream.is_none() {
            // the host is set but the socket isn't opened yet
            self.disconnect_from_remote() ;
            self.try_reconnecting() ;
        }
        else if self.is_connected || self.log_file_path.is_some() {
            // second part of the condition absent from Java code. Allows actually dumping log data
            // when switching from network mode to file mode.
            // FIXME SKIPPING SOME OTHER STUFF

            self.write_messages_to_stream() ;
        }

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "[{:?}] finished processing log queue", thread::current().id()) ;
        }
    }

    fn setup_network_manager_if_required(&mut self) {
        if self.action_receiver.is_none() {
            return ;
        }

        let action_receiver = self.action_receiver.take().unwrap() ;
        let message_sender = self.message_sender.clone() ;

        thread::spawn( move || {
            network_manager::NetworkManager::new(action_receiver, message_sender, network_manager::DefaultBonjourService::new()).run() ;
        }) ;
    }

    fn push_client_info_to_front_of_queue(&mut self) {
        use sys_info ;
        use std::env ;
        use std::ffi::OsStr ;
        use std::path::Path ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "pushing client info to front of queue") ;
        }

        let mut message = LogMessage::new(LogMessageType::ClientInfo, self.get_and_increment_sequence_number()) ;

        match sys_info::os_type() {
            Ok(name) => {


                match sys_info::os_release() {
                    Ok(release) => message.add_string(MessagePartKey::OsVersion, &release),
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
            message.add_string(MessagePartKey::ClientName, &process_name.unwrap()) ;
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

                match new_options.get("remote_host") {
                    Some(host_value) => {
                        host = Some(host_value.clone()) ;
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
                    },
                    None => new_flags |= BROWSE_BONJOUR
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
    }

    pub fn setup_bonjour(&mut self) -> io::Result<()> {
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

            self.action_sender.send(network_manager::NetworkActionMessage::SetupBonjour(service_type.to_string())) ;
        }

        Ok( () )
    }


    pub fn close_bonjour(&mut self) {
        // Nothing to do
    }

    pub fn connect_to_remote(&mut self) -> Result<(), &str> {
        if self.write_stream.is_some() {
            return Err("internal error: remote_socket should be none") ;
        }

        self.close_bonjour() ;

        let remote_host = self.remote_host.as_ref().expect("remote host was none") ;
        let remote_port = self.remote_port.expect("remote port was none") ;
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "connecting to {}:{}", remote_host, remote_port) ;
        }

        let connect_string = format!("{}:{}", remote_host, remote_port) ;
        let stream = match TcpStream::connect(connect_string) {
            Ok(s) => s,
            Err(_) => return Err("error occurred during tcp stream connection")
        } ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "{:?}", &stream) ;
        }
        self.write_stream = Some(WriteStreamWrapper::Tcp(stream)) ;
        if !(self.options & USE_SSL).is_empty() {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "activating SSL connection") ;
            }

            let mut ssl_connector_builder = SslConnector::builder(SslMethod::tls()).unwrap() ;

            ssl_connector_builder.set_verify(openssl::ssl::SslVerifyMode::NONE) ;
            ssl_connector_builder.set_verify_callback(openssl::ssl::SslVerifyMode::NONE, |_,_| { true }) ;

            let connector = ssl_connector_builder.build() ;
            if let WriteStreamWrapper::Tcp(inner_stream) = self.write_stream.take().unwrap() {
                let stream = connector.connect("foo", inner_stream).unwrap();
                self.write_stream = Some(WriteStreamWrapper::Ssl(stream)) ;
                if DEBUG_LOGGER {
                    info!(target:"NSLogger", "opened SSL stream") ;
                }
            }

        }

        self.message_sender.send(HandlerMessageType::ConnectComplete) ;

        Ok( () )
    }

    pub fn disconnect_from_remote(&mut self) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "disconnect_from_remote()") ;
        }

        self.is_connected = false ;
        self.is_connecting = false ;
        self.is_client_info_added = false ;

        if !(self.options & BROWSE_BONJOUR).is_empty() {
            // Otherwise we'll always try to connect to the same host & port, which might change
            // from one connection to the next.
            self.remote_host = None ;
            self.remote_port = None ;
        }

        if self.write_stream.is_none() {
            return ;
        }

        self.write_stream.take() ;
    }

    fn try_reconnecting(&mut self) {
        if self.is_reconnection_scheduled {
            return ;
        }

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "try_reconnecting") ;
        }

        if !(self.options & BROWSE_BONJOUR).is_empty() {
            self.setup_bonjour() ;
        }
        else {
            self.is_reconnection_scheduled = true ;
            // FIXME Should send with 2s delay. Should probably go through the networking thread
            // for handling the reconnect
            self.message_sender.send(HandlerMessageType::TryConnect) ;
        }
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
        if DEBUG_LOGGER && self.write_stream.is_some() {
            info!(target:"NSLogger", "Closing buffer stream") ;
        }

        match self.write_stream.take() {
            Some(mut stream) => stream.flush().unwrap(),
            None => ()

        } ;
        // Letting the file go out of scope will close it

        self.is_client_info_added = false ;
        // otherwise the viewer window won't ever open!
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
        match self.do_write_messages_to_stream() {
            Ok(_) => (),
            Err(e) => {
                if DEBUG_LOGGER {
                    warn!(target:"NSLogger", "Write to stream failed: {:?}", e) ;
                }

                self.disconnect_from_remote() ;
                self.try_reconnecting() ;
            }
        } ;
    }

    fn do_write_messages_to_stream(&mut self) -> io::Result<()> {
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
                        info!(target:"NSLogger", "Writing to {:?}", self.write_stream.as_ref().unwrap()) ;
                        info!(target:"NSLogger", "length: {}", length) ;
                        info!(target:"NSLogger", "bytes: {:?}", &message_bytes[0..cmp::min(length, 40)]) ;
                    }
                }

                {
                    let mut tcp_stream = self.write_stream.as_mut().unwrap() ;

                    try![ tcp_stream.write_all(message_bytes) ] ;
                }

                match message.flush_rx {
                    None => message.flush_tx.send(true).unwrap(),
                    _ => ()
                }
            }


            self.log_messages.remove(0) ;
        }

        Ok( () )
    }
}

impl Drop for LoggerState {
    fn drop(&mut self) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "calling drop for logger state") ;
        }
        self.disconnect_from_remote() ;
        self.action_sender.send(network_manager::NetworkActionMessage::Quit) ;
    }
}
