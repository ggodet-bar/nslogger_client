use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, LazyLock, Mutex},
};

use cfg_if::cfg_if;

/*
 * Module declarations.
 */

mod config;
mod log_message;
mod log_worker;
mod network_manager;
mod reference_counted_runtime;

/*
 * Exports.
 */

#[cfg(test)]
pub(crate) use self::log_message::{LogMessageType, SEQUENCE_NB_OFFSET};
pub(crate) use self::{
    log_message::{LogMessage, MessagePartKey},
    log_worker::{LogWorker, Message},
    reference_counted_runtime::ReferenceCountedRuntime,
};
use crate::nslogger::reference_counted_runtime::RuntimeHandle;
pub use crate::nslogger::{
    config::Config, log_message::Domain, log_worker::ConnectionMode,
    network_manager::BonjourServiceType,
};

/*
 * Constants & global variables.
 */

const DEBUG_LOGGER: bool = true & cfg!(test);

#[cfg(test)]
static START: std::sync::Once = std::sync::Once::new();

static RUNTIME: LazyLock<ReferenceCountedRuntime> =
    LazyLock::new(|| ReferenceCountedRuntime::new().unwrap());

/*
 * Struct declarations.
 */

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("channel was closed or end was dropped")]
    ChannelNotAvailable,
    #[error("IO error")]
    IO(#[from] std::io::Error),
    #[error("invalid file path: {_0}")]
    InvalidPath(String),
}

#[derive(Debug, Default)]
struct InnerSignal {
    state: Mutex<bool>,
    condition: Condvar,
}

/// Shareable struct for parking threads until a condition - whose state is stored internally - is
/// reached. Waiting threads are freed by calling `signal`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Signal(Arc<InnerSignal>);

impl Signal {
    /// Parks calling threads unless the signal condition has been reached.
    pub fn wait(&self) {
        let mut ready = self.0.state.lock().unwrap();
        while !*ready {
            ready = self.0.condition.wait(ready).unwrap();
        }
    }

    /// Sets the internal condition to `true`, and unparks all threads that were waiting on this
    /// condition.
    pub fn signal(&self) {
        let mut ready = self.0.state.lock().unwrap();
        *ready = true;
        self.0.condition.notify_all();
    }
}

pub struct Logger {
    /// Handle to the logger runtime.
    runtime_handle: RuntimeHandle,
    /// Maximum log level.
    filter: log::LevelFilter,
    /// Wait for each message to be sent to the desktop viewer (includes connecting to the viewer).
    flush_messages: bool,
}

impl TryFrom<Config> for Logger {
    type Error = Error;

    fn try_from(
        Config {
            filter,
            mode,
            flush_messages,
        }: Config,
    ) -> Result<Self, Self::Error> {
        let logger = Logger {
            filter,
            flush_messages,
            ..Default::default()
        };
        logger.runtime_handle.switch_connection_mode(mode)?;
        Ok(logger)
    }
}

impl Default for Logger {
    fn default() -> Self {
        if DEBUG_LOGGER {
            cfg_if! {
                if #[cfg(test)] {
                    fn init_test_logger() {
                        START.call_once(|| {
                            env_logger::init();
                        });
                        log::info!("NSLogger client started");
                    }
                }
                else {
                    fn init_test_logger() {}
                }
            }

            init_test_logger();
        }
        Logger {
            runtime_handle: (*RUNTIME).get_handle(),
            filter: log::LevelFilter::Warn,
            flush_messages: false,
        }
    }
}

impl Logger {
    pub fn set_bonjour_service_mode(&mut self, service: BonjourServiceType) -> Result<(), Error> {
        let connection_mode = ConnectionMode::Bonjour(service);
        self.runtime_handle
            .switch_connection_mode(connection_mode)?;
        Ok(())
    }

    pub fn set_remote_host_mode(
        &self,
        host_name: &str,
        host_port: u16,
        use_ssl: bool,
    ) -> Result<(), Error> {
        let connection_mode = ConnectionMode::Tcp(host_name.to_string(), host_port, use_ssl);
        self.runtime_handle
            .switch_connection_mode(connection_mode)?;
        Ok(())
    }

    pub fn set_file_mode(&self, file_path: PathBuf) -> Result<(), Error> {
        let connection_mode = ConnectionMode::File(file_path);
        self.runtime_handle
            .switch_connection_mode(connection_mode)?;
        Ok(())
    }

    pub fn set_message_flushing(&mut self, flush_each_message: bool) {
        self.flush_messages = flush_each_message;
    }

    fn log_and_flush(&self, log_message: LogMessage) {
        if DEBUG_LOGGER {
            log::info!("entering log");
        }
        self.runtime_handle
            .send_and_flush(log_message, self.flush_messages);
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
        let log_message = LogMessage::log(level)
            .with_source_descriptor(filename, line_number, method)
            .with_domain_opt(domain)
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
        let log_message = LogMessage::log(level)
            .with_source_descriptor(filename, line_number, method)
            .with_domain_opt(domain)
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
        let log_message = LogMessage::log(level)
            .with_source_descriptor(filename, line_number, method)
            .with_domain_opt(domain)
            .with_image_data(MessagePartKey::Message, data)
            .freeze();
        self.log_and_flush(log_message);
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
            Some(record.target()),
            None,
            record.level(),
            &format!("{}", record.args()),
        );
    }

    fn flush(&self) {}
}
