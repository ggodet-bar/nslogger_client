use std::sync::{Arc, Mutex};

use log::log;
use tokio::{
    runtime::{Builder, Runtime},
    sync::mpsc,
};

use crate::nslogger::{
    network_manager, network_manager::BonjourServiceType, Error, LogWorker, Message, Signal,
    DEBUG_LOGGER,
};

pub struct LoggerState {
    runtime: Option<Runtime>,
}

impl LoggerState {
    pub fn new(
        message_tx: mpsc::UnboundedSender<Message>,
        message_rx: mpsc::UnboundedReceiver<Message>,
        ready_signal: Signal,
    ) -> Result<Arc<Mutex<Self>>, Error> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        let state = LoggerState {
            runtime: Some(
                Builder::new_multi_thread()
                    .enable_io()
                    .enable_time()
                    .build()?,
            ),
        };
        Self::setup_network_manager(message_tx, command_rx, state.runtime.as_ref().unwrap())?;
        Self::setup_message_worker(
            command_tx,
            message_rx,
            ready_signal,
            state.runtime.as_ref().unwrap(),
        )?;
        Ok(Arc::new(Mutex::new(state)))
    }

    fn setup_network_manager(
        message_tx: mpsc::UnboundedSender<Message>,
        command_rx: mpsc::UnboundedReceiver<BonjourServiceType>,
        runtime: &Runtime,
    ) -> Result<(), Error> {
        runtime.spawn(async move {
            network_manager::NetworkManager::new(command_rx, message_tx)
                .run()
                .await
        });
        Ok(())
    }

    fn setup_message_worker(
        command_tx: mpsc::UnboundedSender<BonjourServiceType>,
        message_rx: mpsc::UnboundedReceiver<Message>,
        ready_signal: Signal,
        runtime: &Runtime,
    ) -> Result<(), Error> {
        runtime.spawn(async {
            LogWorker::new(command_tx, message_rx, ready_signal)
                .run()
                .await
        });
        Ok(())
    }
}

impl Drop for LoggerState {
    fn drop(&mut self) {
        if DEBUG_LOGGER {
            log::info!("calling drop for logger state");
        }
        self.runtime.take().unwrap().shutdown_background();
    }
}
