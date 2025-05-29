use std::sync::{Arc, Mutex};

use log::log;
use tokio::sync::mpsc;

use crate::nslogger::{
    logger_state::{LoggerState, Message},
    message_handler::MessageHandler,
    LoggerOptions, Signal, DEBUG_LOGGER,
};

pub struct MessageWorker {
    pub shared_state: Arc<Mutex<LoggerState>>,
    handler: MessageHandler,
}

impl MessageWorker {
    pub fn new(
        logger_state: Arc<Mutex<LoggerState>>,
        message_rx: mpsc::UnboundedReceiver<Message>,
        ready_signal: Signal,
    ) -> MessageWorker {
        let state_clone = logger_state.clone();
        MessageWorker {
            shared_state: logger_state,
            handler: MessageHandler::new(message_rx, state_clone, ready_signal),
        }
    }

    pub async fn run(&mut self) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Logging thread starting up");
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
            if state.log_file_path.is_some() {
                state.create_buffer_write_stream();
            } else if state.remote_host.is_some() && state.remote_port.is_some() {
                state.connect_to_remote();
            } else if state.options.contains(LoggerOptions::BROWSE_BONJOUR) {
                state.setup_bonjour();
                // Simply triggers an async bonjour service search. The service probably won't be
                // ready when returning from setup_bonjour().
            }
        }
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Starting log event loop");
        }

        // Process messages
        self.handler.run_loop().await;

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Logging thread looper ended");
        }

        // Once loop exists, reset the variable (in case of problem we'll recreate a thread)
        self.shared_state.lock().unwrap().close_bonjour();
        self.shared_state
            .lock()
            .unwrap()
            .close_buffer_write_stream();
        //loggingThread = null;
        //loggingThreadHandler = null;
    }
}
