use std::sync::{Arc, Mutex};

use tokio::{
    runtime::{Builder, Runtime},
    sync::mpsc,
};

use crate::{
    nslogger::{network_manager, Error, LogMessage, LogWorker, Message, Signal, DEBUG_LOGGER},
    ConnectionMode,
};

/// Synchronizes with the logger runtime and handles message sending and flushing.
pub struct RuntimeHandle {
    /// Waits for the log worker to have completed its setup.
    ready: Signal,
    /// Sends finalized messages to the log worker for processing and dispatch.
    message_tx: mpsc::UnboundedSender<Message>,
}

impl RuntimeHandle {
    pub fn switch_connection_mode(&self, mode: ConnectionMode) -> Result<(), Error> {
        self.ready.wait();
        self.message_tx
            .send(Message::SwitchConnection(mode))
            .map_err(|_| Error::ChannelNotAvailable)
    }

    pub fn send_and_flush(&self, log_message: LogMessage, flush_message: bool) {
        self.ready.wait();
        let flush_signal = flush_message.then(Signal::default);
        self.message_tx
            .send(Message::AddLog(log_message, flush_signal.clone()))
            .unwrap();

        let Some(signal) = flush_signal else {
            return;
        };
        if DEBUG_LOGGER {
            log::info!("waiting for message flush");
        }
        signal.wait();
        if DEBUG_LOGGER {
            log::info!("message flush ack received");
        }
    }
}

struct InnerRcRuntime {
    runtime: Option<Runtime>,
    message_tx: mpsc::UnboundedSender<Message>,
}

impl Drop for InnerRcRuntime {
    fn drop(&mut self) {
        if DEBUG_LOGGER {
            log::info!("shutting down runtime");
        }
        let _ = self.message_tx.downgrade();
        self.runtime.take().unwrap().shutdown_background();
    }
}

pub struct ReferenceCountedRuntime(Signal, Arc<Mutex<InnerRcRuntime>>);

impl ReferenceCountedRuntime {
    pub fn new() -> Result<Self, Error> {
        if DEBUG_LOGGER {
            log::info!("initializing logger runtime");
        }
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let ready_signal = Signal::default();

        let runtime = Builder::new_multi_thread()
            .enable_io()
            .enable_time()
            .build()?;
        /*
         * Spawn the network manager.
         */
        runtime.spawn({
            let message_tx = message_tx.clone();
            async move {
                network_manager::NetworkManager::new(command_rx, message_tx.clone())
                    .run()
                    .await
            }
        });
        /*
         * Spawn the log worker.
         */
        runtime.spawn({
            let ready_signal = ready_signal.clone();
            async move {
                LogWorker::new(command_tx, message_rx, ready_signal)
                    .run()
                    .await
            }
        });
        /*
         * Return the runtime.
         */
        Ok(Self(
            ready_signal,
            Arc::new(Mutex::new(InnerRcRuntime {
                runtime: Some(runtime),
                message_tx,
            })),
        ))
    }

    pub fn get_handle(&self) -> RuntimeHandle {
        RuntimeHandle {
            ready: self.0.clone(),
            message_tx: self.1.lock().unwrap().message_tx.clone(),
        }
    }
}
