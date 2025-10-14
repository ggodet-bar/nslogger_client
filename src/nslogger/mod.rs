use std::{
    borrow::Cow,
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
 * Usage.
 */

#[cfg(test)]
pub(crate) use self::log_message::{LogMessageType, SEQUENCE_NB_OFFSET};
pub(crate) use self::{
    log_message::{LogMessage, MessagePartKey},
    log_worker::{LogWorker, Message},
    reference_counted_runtime::ReferenceCountedRuntime,
};
pub use crate::nslogger::{
    config::Config, log_message::Domain, log_worker::ConnectionMode,
    network_manager::BonjourServiceType,
};
use crate::nslogger::{log_message::LogMessageBuilder, reference_counted_runtime::RuntimeHandle};

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

/// High-level builder and dispatcher for log messages.
///
/// This builder constructs message in at most three steps:
///
/// - an optional source description ([`MessageBuilder::with_source_descriptor`])
/// - an optional domain/tag ([`MessageBuilder::with_domain`])
/// - a terminal message, either a string-like object ([`MessageBuilder::message`]), a byte buffer
///   representing an image ([`MessageBuilder::image`]) or a nondescript data byte buffer
///   ([`MessageBuilder::data`]).
///
/// Calling one of the terminal methods will trigger the message's serialization and dispatch to the
/// log worker runtime.
pub struct MessageBuilder<'a>(LogMessageBuilder<'a>, &'a Logger);

impl<'a> MessageBuilder<'a> {
    /// Adds a source file descriptor to the prepared log message.
    pub fn with_source_descriptor(
        self,
        filename: Option<&Path>,
        line_number: Option<u32>,
        method: Option<&'a str>,
    ) -> Self {
        Self(
            self.0
                .with_string_opt(
                    MessagePartKey::FileName,
                    filename.map(|p| p.to_string_lossy().into_owned()),
                )
                .with_int32_opt(MessagePartKey::LineNumber, filename.and(line_number))
                .with_string_opt(MessagePartKey::FunctionName, method),
            self.1,
        )
    }

    /// Adds a `domain` to the prepared log message.
    pub fn with_domain(self, domain: Domain) -> Self {
        Self(
            self.0.with_string(MessagePartKey::Tag, domain.to_string()),
            self.1,
        )
    }

    /// Finalizes the log message with an `image` bytes part and dispatches it to the log worker.
    pub fn image(self, image: &'a [u8]) {
        let log_message = self
            .0
            .with_image_data(MessagePartKey::Message, image)
            .freeze();
        self.1.log_and_flush(log_message);
    }

    /// Finalizes the log message with a `data` part data and dispatches it to the log worker.
    pub fn data(self, data: &'a [u8]) {
        let log_message = self
            .0
            .with_binary_data(MessagePartKey::Message, data)
            .freeze();
        self.1.log_and_flush(log_message);
    }

    /// Finalizes the log message with a string-like `message` and dispatches it to the log worker.
    pub fn message(self, message: impl Into<Cow<'a, str>>) {
        let log_message = self
            .0
            .with_string(MessagePartKey::Message, message)
            .freeze();
        self.1.log_and_flush(log_message);
    }
}

/// A client for building and sending the various types of log messages supported by NSLogger.
///
/// The default `Logger` will use the default connection mode (cf the crate root documentation).
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
    /// Switches the logger to use provided Bonjour `service`, terminating any previous connection
    /// to the desktop application.
    pub fn set_bonjour_service_mode(&mut self, service: BonjourServiceType) -> Result<(), Error> {
        let connection_mode = ConnectionMode::Bonjour(service);
        self.runtime_handle
            .switch_connection_mode(connection_mode)?;
        Ok(())
    }

    /// Switches the logger to connect to the provided `host_name` and `port`, using SSL if defined
    /// in `use_ssl`.This will terminate any previous connection to the desktop application.
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

    /// Switches the logger to dump its messages to the file set at `file_path`. This will terminate
    /// any previous connection to the desktop application.
    pub fn set_file_mode(&self, file_path: PathBuf) -> Result<(), Error> {
        let connection_mode = ConnectionMode::File(file_path);
        self.runtime_handle
            .switch_connection_mode(connection_mode)?;
        Ok(())
    }

    /// Sets whether, after sending a log message, the client should wait for an acknowledgment from
    /// the desktop application (or a disk flush if writing to disk) before proceeding.
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

    /// Starts building a log message with severity `level`.
    ///
    /// Refer to the [`MessageBuilder`] documentation for details on how to complete and dispatch
    /// the log message.
    pub fn log<'a>(&'a self, level: log::Level) -> MessageBuilder<'a> {
        MessageBuilder(LogMessage::log(level), &self)
    }

    /// Log a mark to the desktop viewer.
    ///
    /// Marks are important points that you can jump to directly in the desktop viewer. Message is
    /// optional, if null or empty it will be replaced with the current date / time
    pub fn log_mark(&self, message: Option<&str>) {
        let log_message = LogMessage::mark(message);
        self.log_and_flush(log_message)
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

        self.log(record.level())
            .with_source_descriptor(
                record.file().map(Path::new),
                record.line(),
                Some(record.target()),
            )
            .message(&format!("{}", record.args()));
    }

    fn flush(&self) {}
}
