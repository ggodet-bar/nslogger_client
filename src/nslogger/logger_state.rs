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

        let state = InnerRcRuntime {
            runtime: Some(
                Builder::new_multi_thread()
                    .enable_io()
                    .enable_time()
                    .build()?,
            ),
            message_tx: message_tx.clone(),
        };
        Self::setup_network_manager(message_tx, command_rx, state.runtime.as_ref().unwrap())?;
        Self::setup_message_worker(
            command_tx,
            message_rx,
            ready_signal.clone(),
            state.runtime.as_ref().unwrap(),
        )?;
        Ok(Self(ready_signal, Arc::new(Mutex::new(state))))
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

    pub fn get_signal_and_sender(&self) -> (Signal, mpsc::UnboundedSender<Message>) {
        (self.0.clone(), self.1.lock().unwrap().message_tx.clone())
    }
}
