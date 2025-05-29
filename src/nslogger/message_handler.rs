use std::sync::{Arc, Mutex};

use log::log;
use tokio::sync::mpsc;

use super::log_message::SEQUENCE_NB_OFFSET;
use crate::nslogger::{
    logger_state::{LoggerState, Message},
    Signal, DEBUG_LOGGER,
};

pub struct MessageHandler {
    message_rx: mpsc::UnboundedReceiver<Message>,
    shared_state: Arc<Mutex<LoggerState>>,
    ready_signal: Signal,
    sequence_generator: u32,
}

impl MessageHandler {
    pub fn new(
        message_rx: mpsc::UnboundedReceiver<Message>,
        shared_state: Arc<Mutex<LoggerState>>,
        ready_signal: Signal,
    ) -> MessageHandler {
        MessageHandler {
            sequence_generator: 0,
            message_rx,
            shared_state,
            ready_signal,
        }
    }

    pub async fn run_loop(&mut self) {
        /*
         * We are ready to run. Unpark the waiting threads now
         */
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Message handler ready");
        }
        self.ready_signal.signal();

        while let Some(message) = self.message_rx.recv().await {
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "[{:?}] Received message", std::thread::current().id());
            }

            match message {
                Message::AddLog(mut message, signal) => {
                    /*
                     * Sequence number is set on receiving the message in the handler to
                     * guarantee a strictly monotonic sequence.
                     */
                    message.sequence_number = self.sequence_generator;
                    message.data[SEQUENCE_NB_OFFSET..(SEQUENCE_NB_OFFSET + 4)]
                        .copy_from_slice(&self.sequence_generator.to_ne_bytes());
                    self.sequence_generator += 1;
                    if DEBUG_LOGGER {
                        log::info!(target:"NSLogger", "adding log {} to the queue", message.sequence_number);
                    }

                    let mut local_shared_state = self.shared_state.lock().unwrap();
                    local_shared_state.log_messages.push_back((message, signal));
                    //if local_shared_state.is_connected {
                    local_shared_state.process_log_queue();
                    //}
                }
                Message::OptionChange(new_options) => {
                    if DEBUG_LOGGER {
                        log::info!(target:"NSLogger", "options change received");
                    }

                    self.shared_state
                        .lock()
                        .unwrap()
                        .change_options(new_options);
                }
                Message::TryConnect => self.try_connect(),
                Message::TryConnectBonjour(service_name, host, port) => {
                    if DEBUG_LOGGER {
                        log::info!(target:"NSLogger", "connecting with Bonjour setup service={}, host={}, port={}", service_name, host, port);
                    }

                    let mut local_shared_state = self.shared_state.lock().unwrap();

                    (*local_shared_state).bonjour_service_name = Some(service_name);
                    (*local_shared_state).remote_host = Some(host);
                    (*local_shared_state).remote_port = Some(port);

                    local_shared_state.connect_to_remote();
                }
                Message::Quit => {
                    break;
                }
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

        if local_shared_state.write_stream.is_none()
            && !local_shared_state.is_connecting
            && local_shared_state.remote_host.is_some()
            && local_shared_state.remote_port.is_some()
        {
            local_shared_state.connect_to_remote();
        }
    }
}
