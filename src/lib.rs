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

fn parse_env() -> (log::LevelFilter, ConnectionMode, bool) {
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
    let raw_level = env::var("LOG_LEVEL").unwrap_or("WARN".to_string());
    let level_filter = log::LevelFilter::from_str(&raw_level).unwrap_or(log::LevelFilter::Warn);
    (level_filter, connection_mode, flush_messages)
}

/// Initializes the global logger with a Logger instance.
///
/// This should be called early in the execution of a Rust program, and the
/// global logger may only be initialized once. Future initialization
/// attempts will return an error.
pub fn init() -> Result<(), log::SetLoggerError> {
    let (filter, connection_mode, flush_messages) = parse_env();
    let logger = Logger::with_options(filter, connection_mode, flush_messages).unwrap();
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(filter);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read, time::Duration};

    use log::Level;
    use serial_test::serial;
    use tempfile::NamedTempFile;

    use crate::nslogger::{
        BonjourServiceType, Domain, LogMessageType, Logger, MessagePartKey, MessagePartType,
        SEQUENCE_NB_OFFSET,
    };

    #[test]
    #[serial]
    fn logs_to_file() {
        let tempfile = NamedTempFile::new().expect("temp file");
        let file_path = tempfile.into_temp_path();
        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.set_log_file_path(file_path.to_str().unwrap())
            .expect("setting file path"); // File extension is constrained!!
        let first_msg = "message logged to file";
        log.logm(Some(Domain::App), Level::Warn, first_msg);
        log.logm(
            Some(Domain::DB),
            Level::Warn,
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

    /*
     * NOTE The following tests all rely on NSLogger to be running. As such, they ignored to
     * avoid issues in CI.
     */

    #[test]
    #[serial]
    #[ignore]
    fn creates_logger_instance() {
        let log = Logger::new().expect("logger instance");
        log.logm(Some(Domain::App), Level::Warn, "test");
        log.logm(Some(Domain::DB), Level::Error, "test1");
        log.logm(Some(Domain::DB), Level::Debug, "test2");
        log.logm(Some(Domain::DB), Level::Warn, "test");
        log.logm(Some(Domain::DB), Level::Error, "test1");
        log.logm(Some(Domain::DB), Level::Debug, "test2");
        log.logm(
            Some(Domain::Custom("MyCustomDomain".to_string())),
            Level::Debug,
            "Tag test!",
        );
        log.log("Just a simple message");
        std::thread::sleep(Duration::from_secs(2));
    }

    #[test]
    #[serial]
    #[ignore]
    fn connects_via_bonjour_with_ssl() {
        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warn, "test1");
        log.logm(Some(Domain::App), Level::Warn, "test2");
        log.logm(Some(Domain::App), Level::Warn, "test3");
    }

    #[test]
    #[serial]
    #[ignore]
    fn logs_empty_domain() {
        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.logm(
            Some(Domain::Custom("".to_string())),
            Level::Warn,
            "no domain should appear",
        );
        std::thread::sleep(Duration::from_secs(2));
    }

    #[test]
    #[serial]
    #[ignore]
    fn switches_from_file_to_bonjour() {
        let tempfile = NamedTempFile::new().expect("temp file");

        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.set_log_file_path(tempfile.into_temp_path().to_str().unwrap())
            .expect("setting file path");
        log.logm(Some(Domain::App), Level::Warn, "message logged to file");

        log.set_bonjour_service(BonjourServiceType::Default(false))
            .expect("setting bonjour");
        // no matter the setting
        log.logm(
            Some(Domain::App),
            Level::Warn,
            "message previously logged to file",
        );
    }

    #[test]
    #[serial]
    #[ignore]
    fn switches_from_bonjour_to_file() {
        let tempfile = NamedTempFile::new().expect("temp file");
        let mut log = Logger::new().expect("logger instance");
        log.logm(
            Some(Domain::App),
            Level::Warn,
            "message first logged to Bonjour",
        );

        log.set_message_flushing(true);
        log.set_log_file_path(tempfile.into_temp_path().to_str().unwrap())
            .expect("setting file path"); // File extension is constrained!!
        log.logm(
            Some(Domain::App),
            Level::Warn,
            "message shifted from Bonjour to file",
        );
    }

    #[test]
    #[serial]
    #[ignore]
    fn flushes_log_messages() {
        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warn, "flush test");
        log.logm(Some(Domain::DB), Level::Error, "flush test1");
        log.logm(Some(Domain::DB), Level::Debug, "flush test2");
        log.logm(Some(Domain::DB), Level::Warn, "flush test");
        log.logm(Some(Domain::DB), Level::Error, "flush test1");
        log.logm(Some(Domain::DB), Level::Debug, "flush test2");
    }

    #[test]
    #[serial]
    #[ignore]
    fn logs_mark() {
        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warn, "before mark 1");
        log.logm(Some(Domain::DB), Level::Error, "before mark 2");
        log.log_mark(Some("this is a mark"));
        log.logm(Some(Domain::DB), Level::Debug, "after mark");
    }

    #[test]
    #[serial]
    #[ignore]
    fn logs_empty_mark() {
        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warn, "before mark 1");
        log.logm(Some(Domain::DB), Level::Error, "before mark 2");
        log.log_mark(None);
        log.logm(Some(Domain::DB), Level::Debug, "after mark");
    }

    #[test]
    #[serial]
    #[ignore]
    fn logs_image() {
        use std::{env, fs::File, io::Read};
        let image_path = &env::current_dir().unwrap().join("tests/fixtures/zebra.png");
        let mut file_handle = File::open(image_path).unwrap();
        let mut buffer: Vec<u8> = vec![];

        file_handle.read_to_end(&mut buffer).unwrap();

        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.log_image(None, None, None, None, Level::Warn, &buffer);
    }

    #[test]
    #[serial]
    #[ignore]
    fn logs_binary_data() {
        let bytes: [u8; 8] = [0x6c, 0x6f, 0x67, 0x20, 0x74, 0x65, 0x73, 0x74];
        // should read 'log test'

        let mut log = Logger::new().expect("logger instance");
        log.set_message_flushing(true);
        log.log_data(None, None, None, None, Level::Warn, &bytes);
    }
}
