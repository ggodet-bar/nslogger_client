use std::sync::mpsc ;
use std::sync::{Arc,Mutex} ;
use std::thread ;

use nslogger::logger_state::{ HandlerMessageType, LoggerState } ;
use nslogger::DEBUG_LOGGER ;

pub struct MessageHandler
{
    channel_receiver:mpsc::Receiver<HandlerMessageType>,
    shared_state: Arc<Mutex<LoggerState>>,
}

impl MessageHandler {

    pub fn new(receiver:mpsc::Receiver<HandlerMessageType>, shared_state:Arc<Mutex<LoggerState>>) -> MessageHandler {
        MessageHandler { channel_receiver: receiver, shared_state: shared_state }
    }

    pub fn run_loop(&self) {
        self.shared_state.lock().unwrap().is_handler_running = true  ;
        loop {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "[{:?}] Handler waiting for message", thread::current().id()) ;
            }
            match self.channel_receiver.recv() {
                Ok(message) => {
                    if DEBUG_LOGGER {
                        info!(target:"NSLogger", "[{:?}] Received message: {:?}", thread::current().id(), &message) ;
                    }

                    match message {
                        HandlerMessageType::ADD_LOG(message) => {
                            if DEBUG_LOGGER {
                                info!(target:"NSLogger", "adding log {} to the queue", message.sequence_number) ;
                            }

                            let mut local_shared_state = self.shared_state.lock().unwrap() ;
                            local_shared_state.log_messages.push(message) ;
                            if local_shared_state.is_connected {
                                local_shared_state.process_log_queue() ;
                            }
                        },
                        // NOTE Depends on the LogRecord concept that seems Java-specific
                        //HandlerMessageType::ADD_LOG_RECORD => {
                            //if DEBUG_LOGGER {
                                //info!(target:"NSLogger", "adding LogRecord to the queue") ;
                            //}
                            //let mut local_shared_state = self.shared_state.lock().unwrap() ;
                            //local_shared_state.log_messages.push(LogMessage::new(
                            //if local_shared_state.is_connected {
                                //local_shared_state.process_log_queue() ;
                            //}
                        //},
                        HandlerMessageType::OPTION_CHANGE(new_options) => {
                            if DEBUG_LOGGER {
                                info!(target:"NSLogger", "options change received") ;
                            }

                            self.shared_state.lock().unwrap().change_options(new_options) ;
                        },
                        HandlerMessageType::CONNECT_COMPLETE => {
                            if DEBUG_LOGGER {
                                info!(target:"NSLogger", "connect complete message received") ;
                            }

                            let mut local_shared_state = self.shared_state.lock().unwrap() ;

                            local_shared_state.is_connecting = false ;
                            local_shared_state.is_connected = true ;

                            local_shared_state.process_log_queue() ;
                        },
                        HandlerMessageType::TRY_CONNECT => {
                            let mut local_shared_state = self.shared_state.lock().unwrap() ;
                            if DEBUG_LOGGER {
                                info!(target:"NSLogger",
                                      "try connect message received, remote socket is {:?}, connecting={:?}",
                                      local_shared_state.remote_socket,
                                      local_shared_state.is_connecting) ;
                            }

                            local_shared_state.is_reconnection_scheduled = false ;

                            if local_shared_state.remote_socket.is_none() /* && local_shared_state.write_stream.is_none() */ {
                                if !local_shared_state.is_connecting
                                        && local_shared_state.remote_host.is_some()
                                        && local_shared_state.remote_port.is_some() {
                                    local_shared_state.connect_to_remote() ;
                                }

                            }
                        },

                        HandlerMessageType::QUIT => {
                            break ;
                        }
                        _ => ()
                    }
                },
                Err(e) =>{
                    if DEBUG_LOGGER {
                        warn!(target:"NSLogger", "Error received: {:?}", e) ;
                    }
                    break ;
                }
            }
        } ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "leaving message handler loop") ;
        }
    }
}
