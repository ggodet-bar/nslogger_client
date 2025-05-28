use std::sync::{Arc, Condvar, Mutex};

use log::log;
use tokio::sync::mpsc;

use crate::nslogger::{
    logger_state::{HandlerMessageType, LoggerState},
    message_handler::MessageHandler,
    LoggerOptions, DEBUG_LOGGER,
};

pub struct MessageWorker {
    pub shared_state: Arc<Mutex<LoggerState>>,
    pub message_sender: mpsc::UnboundedSender<HandlerMessageType>,
    handler: MessageHandler,
    ready_cvar: Arc<Condvar>,
}

impl MessageWorker {
    pub fn new(
        logger_state: Arc<Mutex<LoggerState>>,
        ready_cvar: Arc<Condvar>,
        message_sender: mpsc::UnboundedSender<HandlerMessageType>,
        handler_receiver: mpsc::UnboundedReceiver<HandlerMessageType>,
    ) -> MessageWorker {
        let state_clone = logger_state.clone();
        MessageWorker {
            shared_state: logger_state,
            ready_cvar,
            message_sender,
            handler: MessageHandler::new(handler_receiver, state_clone),
        }
    }

    pub fn run(&mut self) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Logging thread starting up");
        }

        // Since we don't have a straightforward way to block the loop (cf Android), we'll setup
        // the connection before releasing the waiting thread(s).

        // Initial setup according to current parameters
        if self.shared_state.lock().unwrap().log_file_path.is_some() {
            self.shared_state
                .lock()
                .unwrap()
                .create_buffer_write_stream();
        } else if {
            let shared_state = self.shared_state.lock().unwrap();
            shared_state.remote_host.is_some() && shared_state.remote_port.is_some()
        } {
            self.shared_state.lock().unwrap().connect_to_remote();
        } else if !(self.shared_state.lock().unwrap().options & LoggerOptions::BROWSE_BONJOUR)
            .is_empty()
        {
            self.shared_state.lock().unwrap().setup_bonjour();
            // Simply triggers an async bonjour service search. The service probably won't be ready
            // when returning from setup_bonjour().
        }

        // We are ready to run. Unpark the waiting threads now
        // (there may be multiple thread trying to start logging at the same time)
        {
            let mut local_shared_state = self.shared_state.lock().unwrap();
            (*local_shared_state).ready = true;
            self.ready_cvar.notify_all();
        }

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Starting log event loop");
        }

        // Process messages
        self.handler.run_loop();

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
