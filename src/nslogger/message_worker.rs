use std::sync::{Arc, Mutex};

use log::log;
use tokio::sync::mpsc;

use crate::nslogger::{
    log_message::SEQUENCE_NB_OFFSET,
    logger_state::{LoggerState, Message},
    Error, Signal, DEBUG_LOGGER,
};

pub struct MessageWorker {
    pub shared_state: Arc<Mutex<LoggerState>>,
    message_rx: mpsc::UnboundedReceiver<Message>,
    ready_signal: Signal,
    sequence_generator: u32,
}

impl MessageWorker {
    pub fn new(
        shared_state: Arc<Mutex<LoggerState>>,
        message_rx: mpsc::UnboundedReceiver<Message>,
        ready_signal: Signal,
    ) -> Self {
        /*
         * NOTE the worker won't process the client info message, hence the very first message
         * is skipped.
         */
        Self {
            sequence_generator: 1,
            message_rx,
            shared_state,
            ready_signal,
        }
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        if DEBUG_LOGGER {
            log::info!("logging thread starting up");
        }

        // Since we don't have a straightforward way to block the loop (cf Android), we'll setup
        // the connection before releasing the waiting thread(s).

        /*
         * Initial setup according to current parameters.
         *
         * NOTE The shared state has to be dropped before the `await`, as the task can be moved
         * around threads at this point.
         */

        {
            let mut state = self.shared_state.lock().unwrap();
            state.setup_connection()?;
        }
        if DEBUG_LOGGER {
            log::info!("starting log event loop");
        }

        self.run_loop().await?;

        if DEBUG_LOGGER {
            log::info!("stopped log event loop");
        }
        self.shared_state
            .lock()
            .unwrap()
            .close_buffer_write_stream()?;
        Ok(())
    }

    pub async fn run_loop(&mut self) -> Result<(), Error> {
        /*
         * We are ready to run. Unpark the waiting threads now
         */
        if DEBUG_LOGGER {
            log::info!("message handler ready");
        }
        self.ready_signal.signal();

        while let Some(message) = self.message_rx.recv().await {
            if DEBUG_LOGGER {
                log::info!("[{:?}] received message", std::thread::current().id());
            }

            match message {
                Message::AddLog(mut message, signal) => {
                    /*
                     * Sequence number is set on receiving the message in the handler to
                     * guarantee a strictly monotonic sequence.
                     */
                    message.sequence_number = self.sequence_generator;
                    message.data[SEQUENCE_NB_OFFSET..(SEQUENCE_NB_OFFSET + 4)]
                        .copy_from_slice(&self.sequence_generator.to_be_bytes());
                    self.sequence_generator += 1;
                    if DEBUG_LOGGER {
                        log::info!("adding log {} to the queue", message.sequence_number);
                    }

                    let mut local_shared_state = self.shared_state.lock().unwrap();
                    local_shared_state.log_messages.push_back((message, signal));
                    local_shared_state.process_log_queue()?;
                }
                Message::ConnectionModeChange(new_mode) => {
                    if DEBUG_LOGGER {
                        log::info!("options change received");
                    }

                    self.shared_state.lock().unwrap().change_options(new_mode)?;
                }
                Message::TryConnectBonjour(service_type, host, port, use_ssl) => {
                    if DEBUG_LOGGER {
                        log::info!("connecting with Bonjour setup service={service_type}, host={host}, port={port}");
                    }

                    let mut local_shared_state = self.shared_state.lock().unwrap();
                    let stream = local_shared_state.connect_to_remote(&host, port, use_ssl)?;
                    local_shared_state.write_stream = Some(stream);
                    local_shared_state.process_log_queue()?;
                }
                Message::Quit => {
                    break;
                }
            }
        }

        if DEBUG_LOGGER {
            log::info!("leaving message handler loop");
        }
        Ok(())
    }
}
