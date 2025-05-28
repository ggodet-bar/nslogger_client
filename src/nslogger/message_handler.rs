use std::sync::{mpsc, Arc, Mutex};

use log::log;

use crate::nslogger::{
    logger_state::{HandlerMessageType, LoggerState},
    DEBUG_LOGGER,
};

pub struct MessageHandler {
    channel_receiver: mpsc::Receiver<HandlerMessageType>,
    shared_state: Arc<Mutex<LoggerState>>,
}

impl MessageHandler {
    pub fn new(
        receiver: mpsc::Receiver<HandlerMessageType>,
        shared_state: Arc<Mutex<LoggerState>>,
    ) -> MessageHandler {
        MessageHandler {
            channel_receiver: receiver,
            shared_state,
        }
    }

    pub fn run_loop(&self) {
        (*self.shared_state.lock().unwrap()).is_handler_running = true;
        for message in &self.channel_receiver {
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "[{:?}] Received message", std::thread::current().id());
            }

            match message {
                HandlerMessageType::AddLog(message) => {
                    if DEBUG_LOGGER {
                        log::info!(target:"NSLogger", "adding log {} to the queue", message.sequence_number);
                    }

                    let mut local_shared_state = self.shared_state.lock().unwrap();
                    local_shared_state.log_messages.push(message);
                    //if local_shared_state.is_connected {
                    local_shared_state.process_log_queue();
                    //}
                }
                HandlerMessageType::OptionChange(new_options) => {
                    if DEBUG_LOGGER {
                        log::info!(target:"NSLogger", "options change received");
                    }

                    self.shared_state
                        .lock()
                        .unwrap()
                        .change_options(new_options);
                }
                HandlerMessageType::ConnectComplete => {
                    if DEBUG_LOGGER {
                        log::info!(target:"NSLogger", "connect complete message received");
                    }

                    let mut local_shared_state = self.shared_state.lock().unwrap();

                    (*local_shared_state).is_connecting = false;
                    (*local_shared_state).is_connected = true;

                    local_shared_state.process_log_queue();
                }
                HandlerMessageType::TryConnect => self.try_connect(),
                HandlerMessageType::TryConnectBonjour(service_name, host, port) => {
                    if DEBUG_LOGGER {
                        log::info!(target:"NSLogger", "connecting with Bonjour setup service={}, host={}, port={}", service_name, host, port);
                    }

                    let mut local_shared_state = self.shared_state.lock().unwrap();

                    (*local_shared_state).bonjour_service_name = Some(service_name);
                    (*local_shared_state).remote_host = Some(host);
                    (*local_shared_state).remote_port = Some(port);

                    local_shared_state.connect_to_remote();
                }

                HandlerMessageType::Quit => {
                    break;
                }
                _ => (),
            }
        }

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "leaving message handler loop");
        }
    }

    fn try_connect(&self) {
        let mut local_shared_state = self.shared_state.lock().unwrap();
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger",
                  "try connect message received, remote socket is {:?}, connecting={:?}",
                  local_shared_state.write_stream,
                  local_shared_state.is_connecting);
        }

        (*local_shared_state).is_reconnection_scheduled = false;

        if local_shared_state.write_stream.is_none() {
            if !local_shared_state.is_connecting
                && local_shared_state.remote_host.is_some()
                && local_shared_state.remote_port.is_some()
            {
                local_shared_state.connect_to_remote();
            }
        }
    }
}
