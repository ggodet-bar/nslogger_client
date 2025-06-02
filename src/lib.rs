//! A client for NSLogger.
//!
//! The `Logger` is essentially a port of the [Java
//! implementation](https://github.com/fpillet/NSLogger/blob/master/Client/Android/client-code/src/com/NSLogger/NSLoggerClient.java),
//! initially designed for Android. Compatible with `log` (obviously without the mark, data and
//! image logging features). Tested on version 1.8.2 of the MacOS NSLogger server.
//!
//! ## TODO:
//!
//! - networking code (setup_bonjour etc.) should probably be run on its own thread, which would
//!   provide simple conditions for writing delayed dispatches and reconnects
//!
//! - opt-out of the networking features (esp. openssl)
//! - builder pattern for logger initialization
//! - possibly some optimizations.

use std::{env, path::PathBuf, str::FromStr};

pub mod nslogger;

use nslogger::ConnectionMode;
pub use nslogger::Logger;

fn parse_env() -> (ConnectionMode, bool) {
    let connection_mode = if let Ok(val) = env::var("LOG_FILENAME") {
        PathBuf::from_str(&val)
            .map(ConnectionMode::File)
            .unwrap_or_default()
    } else {
        let use_ssl = env::var("LOG_USE_SSL")
            .map(|v| v == "1")
            .unwrap_or_default();
        if let Ok(val) = env::var("LOG_REMOTE_HOST") {
            val.split_once(':')
                .map(|(host, port)| {
                    u16::from_str(port)
                        .map(|p| ConnectionMode::Tcp(host.to_string(), p, use_ssl))
                        .unwrap_or_default()
                })
                .unwrap_or_default() // ConnectionMode::Tcp((), (), ())
        } else {
            env::var("LOG_BONJOUR_SERVICE")
                .map(|s| ConnectionMode::Bonjour(nslogger::BonjourServiceType::Custom(s, use_ssl)))
                .unwrap_or_default()
        }
    };
    let flush_messages = env::var("LOG_FLUSH_MESSAGES")
        .map(|v| v == "1")
        .unwrap_or_default();
    (connection_mode, flush_messages)
}

/// Initializes the global logger with a Logger instance.
///
/// This should be called early in the execution of a Rust program, and the
/// global logger may only be initialized once. Future initialization
/// attempts will return an error.
pub fn init() -> Result<(), log::SetLoggerError> {
    log::set_logger(|max_log_level| {
        max_log_level.set(log::LogLevelFilter::Info);
        let (connection_mode, flush_messages) = parse_env();
        let logger = Logger::with_options(connection_mode, flush_messages).unwrap();
        //logger.set_message_flushing(true) ;
        Box::new(logger)
    })
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read, time::Duration};

    use tempfile::NamedTempFile;

    use crate::nslogger::{
        BonjourServiceType, Domain, Level, LogMessageType, Logger, MessagePartKey, MessagePartType,
        SEQUENCE_NB_OFFSET,
    };

    #[test]
    fn connects_via_bonjour_with_ssl() {
        let log = Logger::new().expect("logger instance");
        //log.set_message_flushing(true) ;
        log.logm(Some(Domain::App), Level::Warning, "test1");
        log.logm(Some(Domain::App), Level::Warning, "test2");
        log.logm(Some(Domain::App), Level::Warning, "test3");

        //let ten_millis = time::Duration::from_secs(50);
        //let now = time::Instant::now();

        //thread::sleep(ten_millis);
    }

    #[test]
    fn creates_logger_instance() {
        let log = Logger::new().expect("logger instance");
        log.logm(Some(Domain::App), Level::Warning, "test");
        log.logm(Some(Domain::DB), Level::Error, "test1");
        log.logm(Some(Domain::DB), Level::Debug, "test2");
        log.logm(Some(Domain::DB), Level::Warning, "test");
        log.logm(Some(Domain::DB), Level::Error, "test1");
        log.logm(Some(Domain::DB), Level::Debug, "test2");
        log.logm(
            Some(Domain::Custom("MyCustomDomain".to_string())),
            Level::Debug,
            "Tag test!",
        );
        log.log("Just a simple message");
        std::thread::sleep(Duration::from_secs(6));
    }

    #[test]
    fn logs_empty_domain() {
        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.logm(
            Some(Domain::Custom("".to_string())),
            Level::Warning,
            "no domain should appear",
        );
    }

    #[test]
    fn parses_domain_from_string() {
        use std::str::FromStr;
        assert_eq!(Domain::App, Domain::from_str("App").unwrap());
        assert_eq!(Domain::DB, Domain::from_str("DB").unwrap());
        assert_eq!(
            Domain::Custom("CustomTag".to_string()),
            Domain::from_str("CustomTag").unwrap()
        );
    }

    #[test]
    fn logs_to_file() {
        let tempfile = NamedTempFile::new().expect("temp file");
        let file_path = tempfile.into_temp_path();
        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.set_log_file_path(file_path.to_str().unwrap())
            .expect("setting file path"); // File extension is constrained!!
        let first_msg = "message logged to file";
        log.logm(Some(Domain::App), Level::Warning, first_msg);
        log.logm(
            Some(Domain::DB),
            Level::Warning,
            "other message logged to file",
        );

        let mut file = File::open(file_path).expect("file should exist");
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).expect("file read");
        /*
         * First message should be a client info.
         */
        assert!(buf.len() > 14);
        let msg_size = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
        assert!(buf.len() > msg_size + 4);
        assert_eq!(MessagePartKey::MessageType as u8, buf[6]);
        assert_eq!(MessagePartType::Int32 as u8, buf[7]);
        let msg_type = u32::from_be_bytes(buf[8..12].try_into().unwrap());
        assert_eq!(LogMessageType::ClientInfo as u32, msg_type);
        assert_eq!(
            0,
            u32::from_be_bytes(
                buf[SEQUENCE_NB_OFFSET..(SEQUENCE_NB_OFFSET + 4)]
                    .try_into()
                    .unwrap()
            )
        );
        /*
         * Second message should be a plain log message.
         */
        let next_msg_idx = msg_size + 4;
        let next_msg_size =
            u32::from_be_bytes(buf[next_msg_idx..(next_msg_idx + 4)].try_into().unwrap()) as usize;
        let msg_type = u32::from_be_bytes(
            buf[(next_msg_idx + 8)..(next_msg_idx + 12)]
                .try_into()
                .unwrap(),
        );
        assert_eq!(LogMessageType::Log as u32, msg_type);
        assert_eq!(
            1,
            u32::from_be_bytes(
                buf[(next_msg_idx + SEQUENCE_NB_OFFSET)..(next_msg_idx + SEQUENCE_NB_OFFSET + 4)]
                    .try_into()
                    .unwrap()
            )
        );
        let msg_string_idx = next_msg_idx + next_msg_size + 4 - first_msg.len();
        let msg =
            String::from_utf8(buf[msg_string_idx..(msg_string_idx + first_msg.len())].to_vec())
                .expect("a valid string");
        assert_eq!(first_msg, msg);
        /*
         * Last log message should be yet another plain log message.
         */
        let last_msg_idx = next_msg_idx + next_msg_size + 4;
        let last_msg_size =
            u32::from_be_bytes(buf[last_msg_idx..(last_msg_idx + 4)].try_into().unwrap()) as usize;
        let msg_type = u32::from_be_bytes(
            buf[(last_msg_idx + 8)..(last_msg_idx + 12)]
                .try_into()
                .unwrap(),
        );
        assert_eq!(LogMessageType::Log as u32, msg_type);
        assert_eq!(
            2,
            u32::from_be_bytes(
                buf[(last_msg_idx + SEQUENCE_NB_OFFSET)..(last_msg_idx + SEQUENCE_NB_OFFSET + 4)]
                    .try_into()
                    .unwrap()
            )
        );
        assert_eq!(last_msg_idx + last_msg_size + 4, buf.len());
    }

    #[test]
    fn switches_from_file_to_bonjour() {
        let tempfile = NamedTempFile::new().expect("temp file");

        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.set_log_file_path(tempfile.into_temp_path().to_str().unwrap())
            .expect("setting file path"); // File extension is constrained!!
        log.logm(Some(Domain::App), Level::Warning, "message logged to file");

        log.set_bonjour_service(BonjourServiceType::Default(false))
            .expect("setting bonjour");
        // no matter the setting
        log.logm(
            Some(Domain::App),
            Level::Warning,
            "message previously logged to file",
        );
    }

    #[test]
    fn switches_from_bonjour_to_file() {
        let mut log = Logger::new().expect("logger instance");
        log.logm(
            Some(Domain::App),
            Level::Warning,
            "message first logged to Bonjour",
        );

        log.set_message_flushing(true);
        log.set_log_file_path("/tmp/ns_logger.rawnsloggerdata"); // File extension is constrained!!
        log.logm(
            Some(Domain::App),
            Level::Warning,
            "message shifted from Bonjour to file",
        );
    }

    #[test]
    fn flushes_log_messages() {
        // TODO a better approach would probably be to write a small thread that crashes, with a
        // message that has to be passed before the crash?
        let mut log = Logger::new().expect("logger instance");
        log.set_remote_host("127.0.0.1", 50000, true); // SSL Will be on on the desktop client no matter the setting
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warning, "flush test");
        log.logm(Some(Domain::DB), Level::Error, "flush test1");
        log.logm(Some(Domain::DB), Level::Debug, "flush test2");
        log.logm(Some(Domain::DB), Level::Warning, "flush test");
        log.logm(Some(Domain::DB), Level::Error, "flush test1");
        log.logm(Some(Domain::DB), Level::Debug, "flush test2");
    }

    #[test]
    fn logs_mark() {
        let mut log = Logger::new().expect("logger instance");
        log.set_remote_host("127.0.0.1", 50000, true)
            .expect("setting host");
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warning, "before mark 1");
        log.logm(Some(Domain::DB), Level::Error, "before mark 2");
        log.log_mark(Some("this is a mark"));
        log.logm(Some(Domain::DB), Level::Debug, "after mark");
    }

    #[test]
    fn logs_empty_mark() {
        let mut log = Logger::new().expect("logger instance");
        log.set_remote_host("127.0.0.1", 50000, true);
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warning, "before mark 1");
        log.logm(Some(Domain::DB), Level::Error, "before mark 2");
        log.log_mark(None);
        log.logm(Some(Domain::DB), Level::Debug, "after mark");
    }

    #[test]
    fn logs_image() {
        use std::{env, fs::File, io::Read};
        let image_path = &env::current_dir().unwrap().join("tests/fixtures/zebra.png");
        let mut file_handle = File::open(image_path).unwrap();
        let mut buffer: Vec<u8> = vec![];

        file_handle.read_to_end(&mut buffer).unwrap();

        let mut log = Logger::new().expect("logger instance");
        log.set_remote_host("127.0.0.1", 50000, true);
        log.set_message_flushing(true);
        log.log_image(None, None, None, None, Level::Warning, &buffer);
    }

    #[test]
    fn logs_binary_data() {
        let bytes: [u8; 8] = [0x6c, 0x6f, 0x67, 0x20, 0x74, 0x65, 0x73, 0x74];
        // should read 'log test'

        let mut log = Logger::new().expect("logger instance");
        log.set_remote_host("127.0.0.1", 50000, true);
        log.set_message_flushing(true);
        log.log_data(None, None, None, None, Level::Warning, &bytes);
    }
}
