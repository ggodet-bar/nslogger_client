use std::sync::Arc;

use tokio::{
    runtime::{Builder, Runtime},
    sync::mpsc,
};

use crate::{
    nslogger::{network_manager, Error, LogMessage, LogWorker, Message, Signal},
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

    pub fn send_and_flush(
        &self,
        log_message: LogMessage,
        flush_message: bool,
    ) -> Result<(), Error> {
        self.ready.wait();
        let flush_signal = flush_message.then(Signal::default);
        self.message_tx
            .send(Message::AddLog(log_message, flush_signal.clone()))
            .map_err(|_| Error::ChannelNotAvailable)?;

        let Some(signal) = flush_signal else {
            return Ok(());
        };
        #[cfg(test)]
        log::info!("waiting for message flush");
        signal.wait();
        #[cfg(test)]
        log::info!("message flush ack received");
        Ok(())
    }
}

struct InnerRcRuntime {
    runtime: Option<Runtime>,
    message_tx: mpsc::UnboundedSender<Message>,
}

impl Drop for InnerRcRuntime {
    fn drop(&mut self) {
        #[cfg(test)]
        log::info!("shutting down runtime");
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

pub struct ReferenceCountedRuntime(Signal, Arc<InnerRcRuntime>);

impl ReferenceCountedRuntime {
    /// Initializes the logger runtime. Two tasks are immediately spawned. Failure of either task
    /// will not crash the host application and only trigger an error log message when testing.
    pub fn new() -> Result<Self, Error> {
        #[cfg(test)]
        log::info!("initializing logger runtime");
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
                if let Err(_err) =
                    network_manager::NetworkManager::new(command_rx, message_tx.clone())
                        .run()
                        .await
                {
                    #[cfg(test)]
                    log::error!("logger network manager error: {_err}");
                }
            }
        });
        /*
         * Spawn the log worker.
         */
        runtime.spawn({
            let ready_signal = ready_signal.clone();
            async move {
                if let Err(_err) = LogWorker::new(command_tx, message_rx, ready_signal)
                    .run()
                    .await
                {
                    #[cfg(test)]
                    log::error!("logger error: {_err}");
                }
            }
        });
        /*
         * Return the runtime.
         */
        Ok(Self(
            ready_signal,
            Arc::new(InnerRcRuntime {
                runtime: Some(runtime),
                message_tx,
            }),
        ))
    }

    pub fn get_handle(&self) -> RuntimeHandle {
        RuntimeHandle {
            ready: self.0.clone(),
            message_tx: self.1.message_tx.clone(),
        }
    }
}
