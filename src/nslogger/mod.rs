use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Condvar, Mutex},
};

use bitflags::bitflags;
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

// Exports Level & Domain as part of the public interface
pub use self::log_message::{Domain, Level};
use self::{
    log_message::{LogMessage, LogMessageType, MessagePartKey},
    logger_state::{LoggerState, Message},
    message_worker::MessageWorker,
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

bitflags! {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct LoggerOptions: u16 {
        /// Wait for each message to be sent to the desktop viewer (includes connecting to the viewer)
        const FLUSH_EACH_MESSAGE   = 0b00000001;
        const BROWSE_BONJOUR       = 0b00000010;
        const USE_SSL              = 0b00000100;
        const ROUTE_TO_LOGCAT      = 0b00001000;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("channel was closed or end was dropped")]
    ChannelNotAvailable,
    #[error("IO error")]
    IO(#[from] std::io::Error),
}

pub struct Logger {
    shared_state: Arc<Mutex<LoggerState>>,
    ready_signal: Signal,
    message_tx: mpsc::UnboundedSender<Message>,
}

impl Logger {
    pub fn new() -> Result<Logger, Error> {
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
        });
    }

    pub fn set_bonjour_service(
        &self,
        service_type: Option<&str>,
        service_name: Option<&str>,
        use_ssl: bool,
    ) -> Result<(), Error> {
        if self.ready_signal.is_triggered() {
            let mut properties = HashMap::new();

            if let Some(name_value) = service_name {
                properties.insert("bonjour_service".to_string(), String::from(name_value));
            }

            if let Some(type_value) = service_type {
                properties.insert("bonjour_type".to_string(), String::from(type_value));
            }

            properties.insert(
                "use_ssl".to_string(),
                String::from(if use_ssl { "1" } else { "0" }),
            );

            self.message_tx
                .send(Message::OptionChange(properties))
                .map_err(|_| Error::ChannelNotAvailable)?;
        } else {
            // Worker thread isn't yet setup
            let mut local_shared_state = self.shared_state.lock().unwrap();
            (*local_shared_state).bonjour_service_name =
                service_name.and_then(|v| Some(v.to_string()));
            (*local_shared_state).bonjour_service_type =
                service_type.and_then(|v| Some(v.to_string()));
            (*local_shared_state).options |= LoggerOptions::BROWSE_BONJOUR;

            if use_ssl {
                (*local_shared_state).options = local_shared_state.options | LoggerOptions::USE_SSL;
            } else {
                (*local_shared_state).options = local_shared_state.options - LoggerOptions::USE_SSL;
            }
        }
        Ok(())
    }

    pub fn set_remote_host(
        &self,
        host_name: &str,
        host_port: u16,
        use_ssl: bool,
    ) -> Result<(), Error> {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "set_remote_host host={} port={} use_ssl={}", host_name, host_port, use_ssl);
        }

        if self.ready_signal.is_triggered() {
            let mut properties = HashMap::new();
            properties.insert("remote_host".to_string(), String::from(host_name));
            properties.insert(
                "remote_port".to_string(),
                String::from(format!("{}", host_port)),
            );
            properties.insert(
                "use_ssl".to_string(),
                String::from(if use_ssl { "1" } else { "0" }),
            );

            self.message_tx
                .send(Message::OptionChange(properties))
                .map_err(|_| Error::ChannelNotAvailable)?;
        } else {
            // Worker thread isn't yet setup
            let mut local_shared_state = self.shared_state.lock().unwrap();
            (*local_shared_state).remote_host = Some(String::from(host_name));
            (*local_shared_state).remote_port = Some(host_port);

            if use_ssl {
                (*local_shared_state).options = local_shared_state.options | LoggerOptions::USE_SSL;
            } else {
                (*local_shared_state).options = local_shared_state.options - LoggerOptions::USE_SSL;
            }
        }
        Ok(())
    }

    pub fn set_log_file_path(&self, file_path: &str) -> Result<(), Error> {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "set_log_file_path path={:?}", file_path);
        }

        if self.ready_signal.is_triggered() {
            let mut properties = HashMap::new();
            properties.insert("filename".to_string(), String::from(file_path));

            self.message_tx
                .send(Message::OptionChange(properties))
                .map_err(|_| Error::ChannelNotAvailable)?;
        } else {
            self.shared_state.lock().unwrap().log_file_path =
                Some(PathBuf::from(file_path.to_string()));
        }
        Ok(())
    }

    pub fn set_message_flushing(&self, flush_each_message: bool) {
        let mut local_state = self.shared_state.lock().unwrap();
        if flush_each_message {
            local_state.options |= LoggerOptions::FLUSH_EACH_MESSAGE;
        } else {
            local_state.options -= LoggerOptions::FLUSH_EACH_MESSAGE;
        }
    }

    fn inner_log(&self, log_message: LogMessage) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "entering log");
        }
        self.start_logging_thread_if_needed();
        self.send_and_flush_if_required(log_message);
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

    fn send_and_flush_if_required(&self, log_message: LogMessage) -> Result<(), Error> {
        let needs_flush = self
            .shared_state
            .lock()
            .unwrap()
            .options
            .contains(LoggerOptions::FLUSH_EACH_MESSAGE);
        let flush_signal = needs_flush.then(|| Signal::default());
        self.message_tx
            .send(Message::AddLog(log_message, flush_signal.clone()));

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
