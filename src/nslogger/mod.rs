use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Condvar, LazyLock, Mutex},
};

use cfg_if::cfg_if;
use tokio::sync::mpsc;

const DEBUG_LOGGER: bool = true & cfg!(test);

#[cfg(test)]
use std::sync::Once;

#[cfg(test)]
use env_logger;

#[cfg(test)]
static START: Once = Once::new();

static RUNTIME: LazyLock<ReferenceCountedRuntime> =
    LazyLock::new(|| ReferenceCountedRuntime::new().unwrap());

mod log_message;
mod log_worker;
mod network_manager;
mod reference_counted_runtime;

#[cfg(test)]
pub(crate) use self::log_message::{LogMessageType, SEQUENCE_NB_OFFSET};
pub(crate) use self::{
    log_message::{LogMessage, MessagePartKey},
    log_worker::{LogWorker, Message},
    reference_counted_runtime::ReferenceCountedRuntime,
};
pub use self::{log_worker::ConnectionMode, network_manager::BonjourServiceType};
pub use crate::nslogger::log_message::Domain;

#[derive(Debug, Clone, Default)]
pub struct Signal(Arc<(Mutex<bool>, Condvar)>);

impl Signal {
    pub fn wait(&self) {
        let mut ready = self.0 .0.lock().unwrap();
        while !*ready {
            ready = self.0 .1.wait(ready).unwrap();
        }
    }

    pub fn signal(&self) {
        let mut ready = self.0 .0.lock().unwrap();
        *ready = true;
        self.0 .1.notify_all();
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("channel was closed or end was dropped")]
    ChannelNotAvailable,
    #[error("IO error")]
    IO(#[from] std::io::Error),
    #[error("invalid file path: {_0}")]
    InvalidPath(String),
}

pub struct Logger {
    ready_signal: Signal,
    message_tx: mpsc::UnboundedSender<Message>,
    filter: log::LevelFilter,
    /// Wait for each message to be sent to the desktop viewer (includes connecting to the viewer)
    flush_messages: bool,
}

impl Logger {
    pub fn new() -> Result<Self, Error> {
        if DEBUG_LOGGER {
            cfg_if! {
                if #[cfg(test)] {
                    fn init_test_logger() {
                        START.call_once(|| {
                            env_logger::init() ;
                        }) ;
                        log::info!("NSLogger client started") ;
                    }
                }
                else {
                    fn init_test_logger() {}
                }
            }

            init_test_logger();
        }
        let (ready_signal, message_tx) = (*RUNTIME).get_signal_and_sender();

        Ok(Logger {
            message_tx,
            ready_signal,
            filter: log::LevelFilter::Warn,
            flush_messages: false,
        })
    }

    pub fn with_options(
        filter: log::LevelFilter,
        mode: ConnectionMode,
        flush_messages: bool,
    ) -> Result<Self, Error> {
        let mut logger = Logger::new()?;
        logger.filter = filter;
        logger.flush_messages = flush_messages;
        logger
            .message_tx
            .send(Message::SwitchConnection(mode))
            .map_err(|_| Error::ChannelNotAvailable)?;
        Ok(logger)
    }

    pub fn set_bonjour_service(&mut self, service: BonjourServiceType) -> Result<(), Error> {
        let connection_mode = ConnectionMode::Bonjour(service);
        self.message_tx
            .send(Message::SwitchConnection(connection_mode))
            .map_err(|_| Error::ChannelNotAvailable)?;
        Ok(())
    }

    pub fn set_remote_host(
        &self,
        host_name: &str,
        host_port: u16,
        use_ssl: bool,
    ) -> Result<(), Error> {
        let connection_mode = ConnectionMode::Tcp(host_name.to_string(), host_port, use_ssl);
        self.message_tx
            .send(Message::SwitchConnection(connection_mode))
            .map_err(|_| Error::ChannelNotAvailable)?;
        Ok(())
    }

    pub fn set_log_file_path(&self, file_path: &str) -> Result<(), Error> {
        let connection_mode = ConnectionMode::File(
            PathBuf::from_str(file_path).map_err(|_| Error::InvalidPath(file_path.to_string()))?,
        );
        self.message_tx
            .send(Message::SwitchConnection(connection_mode))
            .map_err(|_| Error::ChannelNotAvailable)?;
        Ok(())
    }

    pub fn set_message_flushing(&mut self, flush_each_message: bool) {
        self.flush_messages = flush_each_message;
    }

    fn log_and_flush(&self, log_message: LogMessage) {
        if DEBUG_LOGGER {
            log::info!("entering log");
        }
        self.start_logging_thread_if_needed();
        self.send_and_flush(log_message);
        if DEBUG_LOGGER {
            log::info!("Exiting log");
        }
    }

    pub fn logl(
        &self,
        filename: Option<&Path>,
        line_number: Option<u32>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: log::Level,
        message: &str,
    ) {
        let log_message = LogMessage::log()
            .with_header(filename, line_number, method, domain, level)
            .with_string(MessagePartKey::Message, message)
            .freeze();
        self.log_and_flush(log_message);
    }

    pub fn logm(&self, domain: Option<Domain>, level: log::Level, message: &str) {
        self.logl(None, None, None, domain, level, message);
    }

    pub fn log(&self, message: &str) {
        self.logm(None, log::Level::Error, message);
    }

    /// Log a mark to the desktop viewer.
    ///
    /// Marks are important points that you can jump to directly in the desktop viewer. Message is
    /// optional, if null or empty it will be replaced with the current date / time
    pub fn log_mark(&self, message: Option<&str>) {
        let log_message = LogMessage::mark(message);
        self.log_and_flush(log_message)
    }

    pub fn log_data(
        &self,
        filename: Option<&Path>,
        line_number: Option<u32>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: log::Level,
        data: &[u8],
    ) {
        let log_message = LogMessage::log()
            .with_header(filename, line_number, method, domain, level)
            .with_binary_data(MessagePartKey::Message, data)
            .freeze();
        self.log_and_flush(log_message)
    }

    pub fn log_image(
        &self,
        filename: Option<&Path>,
        line_number: Option<u32>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: log::Level,
        data: &[u8],
    ) {
        let log_message = LogMessage::log()
            .with_header(filename, line_number, method, domain, level)
            .with_image_data(MessagePartKey::Message, data)
            .freeze();
        self.log_and_flush(log_message);
    }

    fn start_logging_thread_if_needed(&self) {
        if DEBUG_LOGGER {
            log::info!("waiting for worker to be ready");
        }

        self.ready_signal.wait();

        if DEBUG_LOGGER {
            log::info!("worker is ready and running");
        }
    }

    fn send_and_flush(&self, log_message: LogMessage) {
        let flush_signal = self.flush_messages.then(Signal::default);
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

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.filter
            .to_level()
            .map(|l| l <= metadata.level())
            .unwrap_or_default()
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        self.logl(
            record.file().map(Path::new),
            record.line(),
            None,
            Some(Domain::from_str(record.target()).unwrap()),
            record.level(),
            &format!("{}", record.args()),
        );
    }

    fn flush(&self) {}
}
