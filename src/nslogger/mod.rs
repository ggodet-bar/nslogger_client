use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Condvar, Mutex},
    thread,
    thread::spawn,
    time::Duration,
};

use bitflags::bitflags;
use cfg_if::cfg_if;
use log::log;
use tokio::sync::mpsc;

const DEBUG_LOGGER: bool = false & cfg!(test);

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
    logger_state::{HandlerMessageType, LoggerState},
    message_worker::MessageWorker,
};

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

pub struct Logger {
    shared_state: Arc<Mutex<LoggerState>>,
    ready_cvar: Arc<Condvar>,
    message_sender: mpsc::UnboundedSender<HandlerMessageType>,
}

impl Logger {
    pub fn new() -> Logger {
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
        let (message_sender, message_receiver) = mpsc::unbounded_channel();
        let sender_clone = message_sender.clone();

        return Logger {
            message_sender,
            shared_state: Arc::new(Mutex::new(LoggerState::new(sender_clone, message_receiver))),
            ready_cvar: Arc::new(Condvar::new()),
        };
    }

    pub fn set_bonjour_service(
        &mut self,
        service_type: Option<&str>,
        service_name: Option<&str>,
        use_ssl: bool,
    ) {
        if self.shared_state.lock().unwrap().ready {
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

            self.message_sender
                .send(HandlerMessageType::OptionChange(properties));
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
    }

    pub fn set_remote_host(&mut self, host_name: &str, host_port: u16, use_ssl: bool) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "set_remote_host host={} port={} use_ssl={}", host_name, host_port, use_ssl);
        }

        if self.shared_state.lock().unwrap().ready {
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

            self.message_sender
                .send(HandlerMessageType::OptionChange(properties));
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
    }

    pub fn set_log_file_path(&mut self, file_path: &str) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "set_log_file_path path={:?}", file_path);
        }

        if self.shared_state.lock().unwrap().ready {
            let mut properties = HashMap::new();
            properties.insert("filename".to_string(), String::from(file_path));

            self.message_sender
                .send(HandlerMessageType::OptionChange(properties));
        } else {
            self.shared_state.lock().unwrap().log_file_path =
                Some(PathBuf::from(file_path.to_string()));
        }
    }

    pub fn set_message_flushing(&mut self, flush_each_message: bool) {
        let mut local_state = self.shared_state.lock().unwrap();
        if flush_each_message {
            local_state.options |= LoggerOptions::FLUSH_EACH_MESSAGE;
        } else {
            local_state.options -= LoggerOptions::FLUSH_EACH_MESSAGE;
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
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "entering log");
        }
        self.start_logging_thread_if_needed();

        if !self.shared_state.lock().unwrap().is_handler_running {
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "Early return");
            }
            return;
        }

        let mut log_message = LogMessage::with_header(
            LogMessageType::Log,
            self.shared_state
                .lock()
                .unwrap()
                .get_and_increment_sequence_number(),
            filename,
            line_number,
            method,
            domain,
            level,
        );

        log_message.add_string(MessagePartKey::Message, message);

        self.send_and_flush_if_required(log_message);
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Exiting log");
        }
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
    ///
    /// * `message`	optional message
    pub fn log_mark(&self, message: Option<&str>) {
        use chrono;
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "entering log_mark");
        }
        self.start_logging_thread_if_needed();
        if !self.shared_state.lock().unwrap().is_handler_running {
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "Early return");
            }
            return;
        }

        let mut log_message = LogMessage::with_header(
            LogMessageType::Mark,
            self.shared_state
                .lock()
                .unwrap()
                .get_and_increment_sequence_number(),
            None,
            None,
            None,
            None,
            Level::Error,
        );

        let mark_message = match message {
            Some(inner) => inner.to_string(),
            None => {
                let time_now = chrono::Utc::now();

                time_now.format("%b %-d, %-I:%M:%S").to_string()
            }
        };

        log_message.add_string(MessagePartKey::Message, &mark_message);

        self.send_and_flush_if_required(log_message);

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "leaving log_mark");
        }
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
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "entering log_data");
        }

        self.start_logging_thread_if_needed();
        if !self.shared_state.lock().unwrap().is_handler_running {
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "Early return");
            }
            return;
        }

        let mut log_message = LogMessage::with_header(
            LogMessageType::Log,
            self.shared_state
                .lock()
                .unwrap()
                .get_and_increment_sequence_number(),
            filename,
            line_number,
            method,
            domain,
            level,
        );

        log_message.add_binary_data(MessagePartKey::Message, data);

        self.send_and_flush_if_required(log_message);

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "leaving log_data");
        }
    }

    pub fn log_image(
        &mut self,
        filename: Option<&Path>,
        line_number: Option<usize>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: Level,
        data: &[u8],
    ) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "entering log_image");
        }
        self.start_logging_thread_if_needed();
        if !self.shared_state.lock().unwrap().is_handler_running {
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "Early return");
            }
            return;
        }

        let mut log_message = LogMessage::with_header(
            LogMessageType::Log,
            self.shared_state
                .lock()
                .unwrap()
                .get_and_increment_sequence_number(),
            filename,
            line_number,
            method,
            domain,
            level,
        );

        log_message.add_image_data(MessagePartKey::Message, data);

        self.send_and_flush_if_required(log_message);

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "leaving log_image");
        }
    }

    fn start_logging_thread_if_needed(&self) {
        let mut waiting = false;

        {
            let mut local_shared_state = self.shared_state.lock().unwrap();
            if local_shared_state.message_receiver.is_some() {
                local_shared_state.ready_waiters.push(thread::current());
                waiting = true;

                let cloned_state = Arc::clone(&self.shared_state);
                let cloned_cvar = Arc::clone(&self.ready_cvar);
                // NOTE: only clones the pointer reference, not the state itself

                let receiver = local_shared_state.message_receiver.take().unwrap();
                let sender = self.message_sender.clone();
                spawn(move || {
                    MessageWorker::new(cloned_state, cloned_cvar, sender, receiver).run();
                });
            }
        }

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Waiting for worker to be ready");
        }

        while !self.shared_state.lock().unwrap().ready {
            self.ready_cvar.wait_timeout(
                self.shared_state.lock().unwrap(),
                Duration::from_millis(100),
            );

            // We'll keep this loop around in case we find situations where we would need to
            // interrupt the thread.
        }

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "Worker is ready and running");
        }
    }

    fn send_and_flush_if_required(&self, mut log_message: LogMessage) {
        let needs_flush = !(self.shared_state.lock().unwrap().options
            & LoggerOptions::FLUSH_EACH_MESSAGE)
            .is_empty();
        if DEBUG_LOGGER && !needs_flush {
            log::warn!(target:"NSLogger", "no need to flush!!");
        }
        let mut flush_rx: Option<mpsc::UnboundedReceiver<bool>> = None;
        if needs_flush {
            flush_rx = log_message.flush_rx.take();
        }

        self.message_sender
            .send(HandlerMessageType::AddLog(log_message));

        if needs_flush {
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "waiting for message flush");
            }
            flush_rx.unwrap().recv();
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "message flush ack received");
            }
        }
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "calling drop for logger instance");
        }

        self.message_sender.send(HandlerMessageType::Quit);
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::LogMetadata) -> bool {
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
