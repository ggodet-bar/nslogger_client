use std::sync::mpsc ;
use std::sync::{Arc,Mutex} ;
use std::net::ToSocketAddrs ;
use std::time::Duration ;

use tokio_core::reactor::{Core,Timeout} ;
use futures::future::Either ;
use futures::{Stream,Future} ;
use async_dnssd ;
use async_dnssd::Interface ;

use nslogger::logger_state::{ HandlerMessageType, LoggerState } ;
use nslogger::message_handler::MessageHandler ;

use nslogger::DEBUG_LOGGER ;
use nslogger::{USE_SSL, BROWSE_BONJOUR} ;

pub struct MessageWorker
{
    pub shared_state:Arc<Mutex<LoggerState>>,
    pub message_sender:mpsc::Sender<HandlerMessageType>,
    handler:MessageHandler,
}


impl MessageWorker {

    pub fn new(logger_state:Arc<Mutex<LoggerState>>, message_sender:mpsc::Sender<HandlerMessageType>, handler_receiver:mpsc::Receiver<HandlerMessageType>) -> MessageWorker {
        let state_clone = logger_state.clone() ;
        MessageWorker{ shared_state: logger_state,
                       message_sender: message_sender,
                       handler: MessageHandler::new(handler_receiver, state_clone) }
    }

    pub fn run(&mut self) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "Logging thread starting up") ;
        }

        // Since we don't have a straightforward way to block the loop (cf Android), we'll setup
        // the connection before releasing the waiting thread(s).

        // Initial setup according to current parameters
        //if (bufferFile != null)
            //createBufferWriteStream();
        if { let shared_state = self.shared_state.lock().unwrap() ;
             // We're creating a local scope since a double call of lock() will systematically
             // cause a deadlock!

             shared_state.remote_host.is_some()
                && shared_state.remote_port.is_some() } {
            self.shared_state.lock().unwrap().connect_to_remote() ;
        }
        else if !(self.shared_state.lock().unwrap().options & BROWSE_BONJOUR).is_empty() {
            self.setup_bonjour() ;
        }


        // We are ready to run. Unpark the waiting threads now
        // (there may be multiple thread trying to start logging at the same time)
        self.shared_state.lock().unwrap().ready = true ;
        while !self.shared_state.lock().unwrap().ready_waiters.is_empty() {
            self.shared_state.lock().unwrap().ready_waiters.pop().unwrap().unpark() ;
        }

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "Starting log event loop") ;
        }

        // Process messages
        self.handler.run_loop() ;


        if DEBUG_LOGGER {
            info!(target:"NSLogger", "Logging thread looper ended") ;
        }

        // Once loop exists, reset the variable (in case of problem we'll recreate a thread)
        //closeBonjour();
        //loggingThread = null;
        //loggingThreadHandler = null;
    }

    fn setup_bonjour(&mut self) {
        if (self.shared_state.lock().unwrap().options & BROWSE_BONJOUR).is_empty() {
            self.close_bonjour() ;
        }
        else {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "Setting up Bonjour") ;
            }

            let service_type = if (self.shared_state.lock().unwrap().options & USE_SSL).is_empty() {
                "_nslogger._tcp"
            } else {
                "_nslogger-ssl._tcp"
            } ;

            self.shared_state.lock().unwrap().bonjour_service_type = Some(service_type.to_string()) ;
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
                            self.shared_state.lock().unwrap().bonjour_service_name = Some(browse_result.service_name.to_string()) ;
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
                                            self.shared_state.lock().unwrap().remote_host = Some(ip_address) ;
                                            self.shared_state.lock().unwrap().remote_port = Some(resolve_details.port) ;
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

    fn close_bonjour(&self) {
    }
}
