use std::thread::spawn ;
use std::thread ;
use std::sync::mpsc ;
use std::sync::{Arc, Mutex} ;
use std::time::Duration ;
use std::path::Path ;
use std::collections::HashMap ;
use std::str::FromStr ;
use std::path::PathBuf ;

use log ;

const DEBUG_LOGGER:bool = false ;

#[cfg(test)]
use env_logger ;
#[cfg(test)]
use std::sync::{Once, ONCE_INIT} ;

#[cfg(test)]
static START: Once = ONCE_INIT ;

mod log_message ;
mod message_handler ;
mod logger_state ;
mod message_worker ;

// Exports Level & Domain as part of the public interface
pub use self::log_message::{Level,Domain} ;

use self::log_message::{LogMessage,LogMessageType,MessagePartKey} ;
use self::message_worker::MessageWorker ;
use self::logger_state::{LoggerState,HandlerMessageType} ;


bitflags! {
    pub struct LoggerOptions: u16 {
        const FLUSH_EACH_MESSAGE   = 0b00000001 ;
        // If set, NSLogger waits for each message to be sent to the desktop viewer (this includes connecting to the viewer)

        const BROWSE_BONJOUR       = 0b00000010 ;
        const USE_SSL              = 0b00000100 ;
        const ROUTE_TO_LOGCAT      = 0b00001000 ;
    }
}

pub struct Logger {
    shared_state: Arc<Mutex<LoggerState>>,
    message_sender:mpsc::Sender<HandlerMessageType>,
}

impl Logger {

    pub fn new() -> Logger {
        if DEBUG_LOGGER {
            cfg_if! {
                if #[cfg(test)] {
                    fn init_test_logger() {
                        START.call_once(|| {

                            env_logger::init().unwrap() ;
                        }) ;
                        info!(target:"NSLogger", "NSLogger client started") ;
                    }
                }
                else {
                    fn init_test_logger() {}
                }
            }

            init_test_logger() ;
        }
        let (message_sender, message_receiver) = mpsc::channel() ;
        let sender_clone = message_sender.clone() ;

        return Logger{ message_sender: message_sender,
                       shared_state: Arc::new(Mutex::new(LoggerState::new(sender_clone, message_receiver))),
                      } ;
    }

    pub fn set_bonjour_service(&mut self, service_type:Option<&str>, service_name:Option<&str>, use_ssl:bool) {
        if self.shared_state.lock().unwrap().ready {
            let mut properties = HashMap::new() ;

            if let Some(name_value) = service_name {
                properties.insert("bonjour_service".to_string(), String::from(name_value)) ;
            }

            if let Some(type_value) = service_type {
                properties.insert("bonjour_type".to_string(), String::from(type_value)) ;
            }

            properties.insert("use_ssl".to_string(), String::from(if use_ssl { "1" } else { "0" })) ;

            self.message_sender.send(HandlerMessageType::OptionChange(properties)) ;
        }
        else {
            // Worker thread isn't yet setup
            let mut local_shared_state = self.shared_state.lock().unwrap() ;
            local_shared_state.bonjour_service_name = service_name.and_then( |v| Some(v.to_string()) ) ;
            local_shared_state.bonjour_service_type = service_type.and_then( |v| Some(v.to_string()) ) ;

            if use_ssl {
                local_shared_state.options = local_shared_state.options | USE_SSL ;
            } else {
                local_shared_state.options = local_shared_state.options - USE_SSL ;
            }
        }
    }


    pub fn set_remote_host(&mut self, host_name:&str, host_port:u16, use_ssl:bool) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "set_remote_host host={} port={} use_ssl={}", host_name, host_port, use_ssl) ;
        }

        if self.shared_state.lock().unwrap().ready {
            let mut properties = HashMap::new() ;
            properties.insert("remote_host".to_string(), String::from(host_name)) ;
            properties.insert("remote_port".to_string(), String::from(format!("{}", host_port))) ;
            properties.insert("use_ssl".to_string(), String::from(if use_ssl { "1" } else { "0" })) ;

            self.message_sender.send(HandlerMessageType::OptionChange(properties)) ;
        }
        else {
            // Worker thread isn't yet setup
            let mut local_shared_state = self.shared_state.lock().unwrap() ;
            local_shared_state.remote_host = Some(String::from(host_name)) ;
            local_shared_state.remote_port = Some(host_port) ;

            if use_ssl {
                local_shared_state.options = local_shared_state.options | USE_SSL ;
            } else {
                local_shared_state.options = local_shared_state.options - USE_SSL ;
            }
        }
    }

    pub fn set_log_file_path(&mut self, file_path:&str) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "set_log_file_path path={:?}", file_path) ;
        }

        if self.shared_state.lock().unwrap().ready {
            let mut properties = HashMap::new() ;
            properties.insert("filename".to_string(), String::from(file_path)) ;

            self.message_sender.send(HandlerMessageType::OptionChange(properties)) ;
        }
        else {
            self.shared_state.lock().unwrap().log_file_path = Some(PathBuf::from(file_path.to_string())) ;
        }
    }

    pub fn set_message_flushing(&mut self, flush_each_message:bool) {
        let mut local_state = self.shared_state.lock().unwrap() ;
        if flush_each_message {
            local_state.options |= FLUSH_EACH_MESSAGE ;
        } else {
            local_state.options -= FLUSH_EACH_MESSAGE ;
        }
    }

    pub fn logl(&self, filename:Option<&Path>, line_number:Option<usize>, method:Option<&str>, domain:Option<Domain>, level:Level, message:&str) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "entering log") ;
        }
        self.start_logging_thread_if_needed() ;

        if !self.shared_state.lock().unwrap().is_handler_running {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "Early return") ;
            }
            return ;
        }

        let mut log_message = LogMessage::with_header(LogMessageType::Log,
                                                      self.shared_state.lock().unwrap().get_and_increment_sequence_number(),
                                                      filename,
                                                      line_number,
                                                      method,
                                                      domain,
                                                      level) ;


        log_message.add_string(MessagePartKey::Message, message) ;

        self.send_and_flush_if_required(log_message) ;
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "Exiting log") ;
        }
    }

    pub fn logm(&self, domain: Option<Domain>, level: Level, message:&str) {
        self.logl(None, None, None, domain, level, message) ;
    }

    pub fn log(&self, message:&str) {
        self.logm(None, Level::Error, message) ;
    }

	/// Log a mark to the desktop viewer.
    ///
    /// Marks are important points that you can jump to directly in the desktop viewer. Message is
    /// optional, if null or empty it will be replaced with the current date / time
    ///
	/// * `message`	optional message
	///
    pub fn log_mark(&self, message:Option<&str>) {
        use chrono ;
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "entering log_mark") ;
        }
        self.start_logging_thread_if_needed() ;
        if !self.shared_state.lock().unwrap().is_handler_running {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "Early return") ;
            }
            return ;
        }

        let mut log_message = LogMessage::with_header(LogMessageType::Mark,
                                                      self.shared_state.lock().unwrap().get_and_increment_sequence_number(),
                                                      None,
                                                      None,
                                                      None,
                                                      None,
                                                      Level::Error) ;

        let mark_message = match message {
            Some(inner) => inner.to_string(),
            None => {
                let time_now = chrono::Utc::now() ;

                time_now.format("%b %-d, %-I:%M:%S").to_string()
            }
        } ;

        log_message.add_string(MessagePartKey::Message, &mark_message) ;

        self.send_and_flush_if_required(log_message) ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "leaving log_mark") ;
        }
    }

    pub fn log_data(&self, filename:Option<&Path>, line_number:Option<usize>, method:Option<&str>,
                     domain:Option<Domain>, level:Level, data:&[u8]) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "entering log_data") ;
        }

        self.start_logging_thread_if_needed() ;
        if !self.shared_state.lock().unwrap().is_handler_running {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "Early return") ;
            }
            return ;
        }

        let mut log_message = LogMessage::with_header(LogMessageType::Log,
                                                      self.shared_state.lock().unwrap().get_and_increment_sequence_number(),
                                                      filename,
                                                      line_number,
                                                      method,
                                                      domain,
                                                      level) ;

        log_message.add_binary_data(MessagePartKey::Message, data) ;

        self.send_and_flush_if_required(log_message) ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "leaving log_data") ;
        }
    }

    pub fn log_image(&mut self, filename:Option<&Path>, line_number:Option<usize>, method:Option<&str>,
                     domain:Option<Domain>, level:Level, data:&[u8]) {

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "entering log_image") ;
        }
        self.start_logging_thread_if_needed() ;
        if !self.shared_state.lock().unwrap().is_handler_running {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "Early return") ;
            }
            return ;
        }

        let mut log_message = LogMessage::with_header(LogMessageType::Log,
                                                      self.shared_state.lock().unwrap().get_and_increment_sequence_number(),
                                                      filename,
                                                      line_number,
                                                      method,
                                                      domain,
                                                      level) ;

        log_message.add_image_data(MessagePartKey::Message, data) ;

        self.send_and_flush_if_required(log_message) ;

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "leaving log_image") ;
        }
    }

    fn start_logging_thread_if_needed(&self) {
        let mut waiting = false ;

        {
            let mut local_shared_state = self.shared_state.lock().unwrap() ;
            match local_shared_state.message_receiver {
                Some(_) => {
                    local_shared_state.ready_waiters.push(thread::current()) ;
                    let cloned_state = self.shared_state.clone() ;

                    let receiver = local_shared_state.message_receiver.take().unwrap() ;
                    let sender = self.message_sender.clone() ;
                    spawn( move || {
                        MessageWorker::new(cloned_state, sender, receiver).run() ;
                    }) ;
                    waiting = true ;

                },
                _ => ()

            } ;
        }


        if DEBUG_LOGGER {
            info!(target:"NSLogger", "Waiting for worker to be ready") ;
        }

        while !self.shared_state.lock().unwrap().ready {
            if !waiting {
                self.shared_state.lock().unwrap().ready_waiters.push(thread::current()) ;
                waiting = true ;
            }

            thread::park_timeout(Duration::from_millis(100)) ;
            //if (Thread.interrupted())
            //   Thread.currentThread().interrupt();

        }

        if DEBUG_LOGGER {
            info!(target:"NSLogger", "Worker is ready and running") ;
        }
    }

    fn send_and_flush_if_required(& self, mut log_message:LogMessage) {
        let needs_flush = !(self.shared_state.lock().unwrap().options & FLUSH_EACH_MESSAGE).is_empty() ;
        if DEBUG_LOGGER && !needs_flush {
            warn!(target:"NSLogger", "no need to flush!!") ;
        }
        let mut flush_rx:Option<mpsc::Receiver<bool>> = None ;
        if needs_flush {
            flush_rx = log_message.flush_rx.take() ;
        }

        self.message_sender.send(HandlerMessageType::AddLog(log_message)) ;

        if needs_flush {
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "waiting for message flush") ;
            }
            flush_rx.unwrap().recv() ;
            if DEBUG_LOGGER {
                info!(target:"NSLogger", "message flush ack received") ;
            }
        }
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        if DEBUG_LOGGER {
            info!(target:"NSLogger", "calling drop for logger instance") ;
        }

        self.message_sender.send(HandlerMessageType::Quit) ;

    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata:&log::LogMetadata) -> bool {
        true
    }

    fn log(&self, record:&log::LogRecord) {
        if !self.enabled(record.metadata()) {
            return ;
        }

        self.logl(Some(Path::new(record.location().file())),
                   Some(record.location().line() as usize),
                   None,
                   Some(Domain::from_str(record.target()).unwrap()),
                   Level::from_log_level(record.level()),
                   &format!("{}", record.args())) ;
    }
}

unsafe impl Sync for Logger {}

unsafe impl Send for Logger {}


