use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Condvar, Mutex},
};

use cfg_if::cfg_if;
use chrono;
use log::log;
use tokio::sync::mpsc;

const DEBUG_LOGGER: bool = true & cfg!(test);

#[cfg(test)]
use std::sync::Once;

#[cfg(test)]
use env_logger;

#[cfg(test)]
static START: Once = Once::new();

mod log_message;
mod logger_state;
mod message_handler;
mod message_worker;
mod network_manager;

pub use self::log_message::{Domain, Level};
#[cfg(test)]
pub(crate) use self::log_message::{MessagePartType, SEQUENCE_NB_OFFSET};
pub(crate) use self::{
    log_message::{LogMessage, LogMessageType, MessagePartKey},
    logger_state::{ConnectionMode, LoggerState, Message},
    message_worker::MessageWorker,
    network_manager::BonjourServiceType,
};

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

    pub fn is_triggered(&self) -> bool {
        *self.0 .0.lock().unwrap()
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
    shared_state: Arc<Mutex<LoggerState>>,
    ready_signal: Signal,
    message_tx: mpsc::UnboundedSender<Message>,
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
                        log::info!(target:"NSLogger", "NSLogger client started") ;
                    }
                }
                else {
                    fn init_test_logger() {}
                }
            }

            init_test_logger();
        }
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let ready_signal = Signal::default();

        return Ok(Logger {
            shared_state: LoggerState::new(message_tx.clone(), message_rx, ready_signal.clone())?,
            message_tx,
            ready_signal,
            flush_messages: false,
        });
    }

    pub fn with_options(mode: ConnectionMode, flush_messages: bool) -> Result<Self, Error> {
        let mut logger = Logger::new()?;
        logger.shared_state.lock().unwrap().connection_mode = mode;
        logger.flush_messages = flush_messages;
        Ok(logger)
    }

    pub fn set_bonjour_service(&mut self, service: BonjourServiceType) -> Result<(), Error> {
        let connection_mode = ConnectionMode::Bonjour(service);
        self.message_tx
            .send(Message::ConnectionModeChange(connection_mode))
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
            .send(Message::ConnectionModeChange(connection_mode))
            .map_err(|_| Error::ChannelNotAvailable)?;
        Ok(())
    }

    pub fn set_log_file_path(&self, file_path: &str) -> Result<(), Error> {
        let connection_mode = ConnectionMode::File(
            PathBuf::from_str(file_path).map_err(|_| Error::InvalidPath(file_path.to_string()))?,
        );
        self.message_tx
            .send(Message::ConnectionModeChange(connection_mode))
            .map_err(|_| Error::ChannelNotAvailable)?;
        Ok(())
    }

    pub fn set_message_flushing(&mut self, flush_each_message: bool) {
        self.flush_messages = flush_each_message;
    }

    fn inner_log(&self, log_message: LogMessage) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "entering log");
        }
        self.start_logging_thread_if_needed();
        self.send_and_flush(log_message);
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Exiting log");
        }
    }

    pub fn logl(
        &self,
        filename: Option<&Path>,
        line_number: Option<usize>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: Level,
        message: &str,
    ) {
        let mut log_message = LogMessage::with_header(
            LogMessageType::Log,
            filename,
            line_number,
            method,
            domain,
            level,
        );
        log_message.add_string(MessagePartKey::Message, message);
        self.inner_log(log_message);
    }

    pub fn logm(&self, domain: Option<Domain>, level: Level, message: &str) {
        self.logl(None, None, None, domain, level, message);
    }

    pub fn log(&self, message: &str) {
        self.logm(None, Level::Error, message);
    }

    /// Log a mark to the desktop viewer.
    ///
    /// Marks are important points that you can jump to directly in the desktop viewer. Message is
    /// optional, if null or empty it will be replaced with the current date / time
    pub fn log_mark(&self, message: Option<&str>) {
        let mut log_message =
            LogMessage::with_header(LogMessageType::Mark, None, None, None, None, Level::Error);

        let mark_message = message.map(|msg| msg.to_string()).unwrap_or_else(|| {
            let time_now = chrono::Utc::now();
            time_now.format("%b %-d, %-I:%M:%S").to_string()
        });
        log_message.add_string(MessagePartKey::Message, &mark_message);
        self.inner_log(log_message)
    }

    pub fn log_data(
        &self,
        filename: Option<&Path>,
        line_number: Option<usize>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: Level,
        data: &[u8],
    ) {
        let mut log_message = LogMessage::with_header(
            LogMessageType::Log,
            filename,
            line_number,
            method,
            domain,
            level,
        );
        log_message.add_binary_data(MessagePartKey::Message, data);
        self.inner_log(log_message)
    }

    pub fn log_image(
        &self,
        filename: Option<&Path>,
        line_number: Option<usize>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: Level,
        data: &[u8],
    ) {
        let mut log_message = LogMessage::with_header(
            LogMessageType::Log,
            filename,
            line_number,
            method,
            domain,
            level,
        );
        log_message.add_image_data(MessagePartKey::Message, data);
        self.inner_log(log_message);
    }

    fn start_logging_thread_if_needed(&self) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Waiting for worker to be ready");
        }

        self.ready_signal.wait();

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Worker is ready and running");
        }
    }

    fn send_and_flush(&self, log_message: LogMessage) -> Result<(), Error> {
        let flush_signal = self.flush_messages.then(|| Signal::default());
        self.message_tx
            .send(Message::AddLog(log_message, flush_signal.clone()))
            .map_err(|_| Error::ChannelNotAvailable)?;

        let Some(signal) = flush_signal else {
            return Ok(());
        };
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "waiting for message flush");
        }
        signal.wait();
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "message flush ack received");
        }
        Ok(())
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "calling drop for logger instance");
        }

        self.message_tx.send(Message::Quit);
    }
}

impl log::Log for Logger {
    fn enabled(&self, _: &log::LogMetadata) -> bool {
        true
    }

    fn log(&self, record: &log::LogRecord) {
        if !self.enabled(record.metadata()) {
            return;
        }

        self.logl(
            Some(Path::new(record.location().file())),
            Some(record.location().line() as usize),
            None,
            Some(Domain::from_str(record.target()).unwrap()),
            Level::from_log_level(record.level()),
            &format!("{}", record.args()),
        );
    }
}

unsafe impl Sync for Logger {}

unsafe impl Send for Logger {}
