use std::io ;
use std::io::Write ;
use std::thread ;
use std::thread::Thread ;
use std::sync::mpsc ;
use std::sync::atomic::{AtomicU32, Ordering} ;
use std::net::TcpStream ;
use std::collections::HashMap ;

use openssl ;
use openssl::ssl::{SslMethod, SslConnectorBuilder, SslStream} ;

use nslogger::log_message::{LogMessage, LogMessageType} ;

use nslogger::DEBUG_LOGGER ;
use nslogger::LoggerOptions ;
use nslogger::USE_SSL ;

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
pub enum SocketWrapper {
    Tcp(TcpStream),
    Ssl(SslStream<TcpStream>),
}

impl SocketWrapper {
    pub fn write_all(&mut self, buf:&[u8]) -> io::Result<()> {
        match *self {
            SocketWrapper::Tcp(ref mut stream) => return stream.write_all(buf),
            SocketWrapper::Ssl(ref mut stream) => return stream.write_all(buf)
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

    /// the remote socket we're talking to
    pub remote_socket:Option<SocketWrapper>,

    /// file or socket output stream
    //pub write_stream:Option<Write + 'static:std::marker::Sized>,

    pub next_sequence_numbers:AtomicU32,
    pub log_messages:Vec<LogMessage>,
    pub message_sender:mpsc::Sender<HandlerMessageType>,
    pub message_receiver:Option<mpsc::Receiver<HandlerMessageType>>,
}

impl LoggerState
{
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

        if self.is_connected {
            // FIXME SKIPPING SOME OTHER STUFF

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
                        let mut tcp_stream = self.remote_socket.as_mut().unwrap() ;
                        tcp_stream.write_all(message_bytes).expect("Write to TCP stream failed") ;
                    }

                    match message.flush_rx {
                        None => message.flush_tx.send(true).unwrap(),
                        _ => ()
                    }
                }


                self.log_messages.remove(0) ;
            }
        }

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "[{:?}] finished processing log queue", thread::current().id()) ;
        }
    }

    fn push_client_info_to_front_of_queue(&mut self) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "pushing client info to front of queue") ;
        }

        let message = LogMessage::new(LogMessageType::CLIENT_INFO, self.get_and_increment_sequence_number()) ;
        self.log_messages.insert(0, message) ;
        self.is_client_info_added = true ;
    }

    pub fn change_options(&mut self, new_options:HashMap<String, String>) {

        // FIXME TEMP!!!
        self.connect_to_remote() ;
    }

    pub fn connect_to_remote(&mut self) -> Result<(), &str> {
        //if self.write_stream.is_some() {
            //return Err("internal error: write_stream should be none") ;
        //}
        if self.remote_socket.is_some() {
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
        self.remote_socket = Some(SocketWrapper::Tcp(stream)) ;
        if !(self.options | USE_SSL).is_empty() {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "activating SSL connection") ;
            }

            let mut ssl_connector_builder = SslConnectorBuilder::new(SslMethod::tls()).unwrap() ;

            ssl_connector_builder.builder_mut().set_verify(openssl::ssl::SSL_VERIFY_NONE) ;
            ssl_connector_builder.builder_mut().set_verify_callback(openssl::ssl::SSL_VERIFY_NONE, |_,_| { true }) ;

            let connector = ssl_connector_builder.build() ;
            if let SocketWrapper::Tcp(inner_stream) = self.remote_socket.take().unwrap() {
                let stream = connector.danger_connect_without_providing_domain_for_certificate_verification_and_server_name_indication(inner_stream).unwrap();
                self.remote_socket = Some(SocketWrapper::Ssl(stream)) ;
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

    pub fn get_and_increment_sequence_number(&mut self) -> u32 {
        return self.next_sequence_numbers.fetch_add(1, Ordering::SeqCst) ;
    }
}
