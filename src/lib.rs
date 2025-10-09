// Licensed under the MIT license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed except according to those terms.

//! A client for the MacOS [NSLogger](https://github.com/fpillet/NSLogger) log viewer.
//!
//!## Examples
//!
//!Using the `log` crate facade
//!
//!```rust
//! use log::info;
//!
//! nslogger::init();
//! info!("this is an NSLogger message");
//! ```
//!
//!Using the `nslogger_client` API:
//!
//!```rust
//! use nslogger::{Logger, Domain};
//! use log::Level;
//!
//! let log = Logger::new().expect("a logger instance");
//! log.logm(Some(Domain::App), Level::Info, "starting application");
//! log.log_mark(None); // will display a mark with no label in the NSLogger viewer
//! log.logm(Some(Domain::App), Level::Info, "leaving application");
//! ```
//!
//!## NOT supported:
//!
//!At the moment there are no plans to add support for the following NSLogger features:
//!
//! - message blocks
//! - client disconnects
use std::{env, path::PathBuf, str::FromStr};

mod nslogger;

use nslogger::ConnectionMode;
pub use nslogger::{Domain, Logger};

/// Parses the environment variables to identify the max logging level, the type of connection to
/// NSLogger (or the log file path), and whether the logger should wait for each message to be
/// handled before returning from the log calls.
fn parse_env() -> (log::LevelFilter, ConnectionMode, bool) {
    let connection_mode = if let Ok(val) = env::var("NSLOG_FILENAME") {
        PathBuf::from_str(&val)
            .map(ConnectionMode::File)
            .unwrap_or_default()
    } else {
        let use_ssl = env::var("NSLOG_USE_SSL").map(|v| v != "0").unwrap_or(true);
        if let Ok(val) = env::var("NSLOG_HOST") {
            val.split_once(':')
                .map(|(host, port)| {
                    u16::from_str(port)
                        .map(|p| ConnectionMode::Tcp(host.to_string(), p, use_ssl))
                        .unwrap_or_default()
                })
                .unwrap_or_default() // ConnectionMode::Tcp((), (), ())
        } else {
            let service_type = env::var("NSLOG_BONJOUR_SERVICE")
                .map(|s| nslogger::BonjourServiceType::Custom(s, use_ssl))
                .unwrap_or(nslogger::BonjourServiceType::Default(use_ssl));
            ConnectionMode::Bonjour(service_type)
        }
    };
    let flush_messages = env::var("NSLOG_FLUSH")
        .map(|v| v == "1")
        .unwrap_or_default();
    let raw_level = env::var("NSLOG_LEVEL").unwrap_or("WARN".to_string());
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

    use super::*;
    use crate::nslogger::{
        BonjourServiceType, ConnectionMode, Domain, LogMessageType, Logger, MessagePartKey,
        MessagePartType, SEQUENCE_NB_OFFSET,
    };

    #[test]
    #[serial]
    fn parses_default_env() {
        assert_eq!(
            (log::LevelFilter::Warn, ConnectionMode::default(), false),
            parse_env()
        )
    }

    #[test]
    #[serial]
    fn parses_file_path_from_env() {
        unsafe {
            env::set_var("NSLOG_FILENAME", "/tmp/file_output.log");
        }
        assert_eq!(
            (
                log::LevelFilter::Warn,
                ConnectionMode::File(PathBuf::from("/tmp/file_output.log")),
                false
            ),
            parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_FILENAME");
        }
    }

    #[test]
    #[serial]
    fn parses_log_level_from_env() {
        unsafe {
            env::set_var("NSLOG_LEVEL", "INFO");
        }
        assert_eq!(
            (log::LevelFilter::Info, ConnectionMode::default(), false),
            parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_LEVEL");
        }
    }

    #[test]
    #[serial]
    fn disables_bonjour_ssl_from_env() {
        unsafe {
            env::set_var("NSLOG_USE_SSL", "0");
        }
        assert_eq!(
            (
                log::LevelFilter::Warn,
                ConnectionMode::Bonjour(BonjourServiceType::Default(false)),
                false
            ),
            parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_USE_SSL");
        }
    }

    #[test]
    #[serial]
    fn sets_remote_host_from_env() {
        unsafe {
            env::set_var("NSLOG_HOST", "127.0.0.1:50000");
        }
        assert_eq!(
            (
                log::LevelFilter::Warn,
                ConnectionMode::Tcp("127.0.0.1".to_string(), 50000, true),
                false
            ),
            parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_HOST");
        }
    }

    #[test]
    #[serial]
    fn sets_message_flushing_from_env() {
        unsafe {
            env::set_var("NSLOG_FLUSH", "1");
        }
        assert_eq!(
            (log::LevelFilter::Warn, ConnectionMode::default(), true),
            parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_FLUSH");
        }
    }

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
        assert!(buf.len() > SEQUENCE_NB_OFFSET + 4);
        let msg_size = u32::from_be_bytes(buf[0..4].try_into().unwrap()) as usize;
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
        let next_msg_offset = msg_size + 4;
        let next_msg_size = u32::from_be_bytes(
            buf[next_msg_offset..(next_msg_offset + 4)]
                .try_into()
                .unwrap(),
        ) as usize;
        let next_msg_part_count = u16::from_be_bytes(
            buf[(next_msg_offset + 4)..(next_msg_offset + 6)]
                .try_into()
                .unwrap(),
        );
        assert_eq!(8, next_msg_part_count);
        let msg_type = u32::from_be_bytes(
            buf[(next_msg_offset + 8)..(next_msg_offset + 12)]
                .try_into()
                .unwrap(),
        );
        assert_eq!(LogMessageType::Log as u32, msg_type);
        assert_eq!(
            1,
            u32::from_be_bytes(
                buf[(next_msg_offset + SEQUENCE_NB_OFFSET)
                    ..(next_msg_offset + SEQUENCE_NB_OFFSET + 4)]
                    .try_into()
                    .unwrap()
            )
        );
        let msg_string_offset = next_msg_offset + next_msg_size + 4 - first_msg.len();
        let msg = String::from_utf8(
            buf[msg_string_offset..(msg_string_offset + first_msg.len())].to_vec(),
        )
        .expect("a valid string");
        assert_eq!(first_msg, msg);
        /*
         * Last log message should be yet another plain log message.
         */
        let last_msg_offset = next_msg_offset + next_msg_size + 4;
        let last_msg_size = u32::from_be_bytes(
            buf[last_msg_offset..(last_msg_offset + 4)]
                .try_into()
                .unwrap(),
        ) as usize;
        let msg_type = u32::from_be_bytes(
            buf[(last_msg_offset + 8)..(last_msg_offset + 12)]
                .try_into()
                .unwrap(),
        );
        assert_eq!(LogMessageType::Log as u32, msg_type);
        assert_eq!(
            2,
            u32::from_be_bytes(
                buf[(last_msg_offset + SEQUENCE_NB_OFFSET)
                    ..(last_msg_offset + SEQUENCE_NB_OFFSET + 4)]
                    .try_into()
                    .unwrap()
            )
        );
        assert_eq!(last_msg_offset + last_msg_size + 4, buf.len());
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
