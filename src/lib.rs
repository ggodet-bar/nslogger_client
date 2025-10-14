// Licensed under the MIT license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed except according to those terms.

//! A client for the MacOS [NSLogger](https://github.com/fpillet/NSLogger) log viewer.
//!
//! ## Logger configuration
//!
//! By default, `nslogger_client` connects to the default application using the default Bonjour
//! service handle with SSL (`_nslogger-ssl._tcp.`) with no message flushing.
//!
//! `nslogger_client` may be configured via the following environment variables:
//!
//! - `NSLOG_LEVEL` - maximum log level, with `TRACE` being the highest value and `ERROR` the lowest
//!   (cf [`log::Level`]);
//! - `NSLOG_FLUSH` - whether the logger should wait for the desktop application to send an ack
//!   signal after each message;
//! - `NSLOG_FILENAME` - switches the logger to print messages to the given filepath **instead** of
//!   the destkop application;
//! - `NSLOG_BONJOUR_SERVICE` - switches the logger to connect to the desktop application using the
//!   given Bonjour handler;
//! - `NSLOG_USE_SSL` - switches the logger to connect to the desktop application (whether via
//!   Bonjour or a direct TCP connection) with or without SSL (`0` disables SSL, any other value
//!   enables it);
//! - `NSLOG_HOST` - switches the logger to connect to the desktop application via TCP using the
//!   `<address>:<port>` format. Domain name resolution is supported;
//!
//! ## Examples
//!
//! `nslogger` client may be used either as a logging provider for the [`log`] facade, similarly to
//! [`env_logger`](https://docs.rs/env_logger/latest/env_logger), or as a standalone logger.
//!
//!
//! ### Using the `log` crate facade
//!
//!```rust
//! use log::info;
//!
//! nslogger::init();
//! info!("this is an NSLogger message");
//! ```
//!
//! ### Using the `nslogger_client` API
//!
//!```rust
//! use nslogger::{Logger, Domain};
//! use log::Level;
//! # use std::{env, fs::File, io::Read};
//! # let image_path = &env::current_dir().unwrap().join("tests/fixtures/zebra.png");
//! # let mut file_handle = File::open(image_path).unwrap();
//! # let mut image_buffer = Vec::new();
//! # file_handle.read_to_end(&mut image_buffer).unwrap();
//! let mut logger = Logger::default();
//! # logger.set_message_flushing(true);
//! logger.log(Level::Info).with_domain(Domain::App).message("starting application");
//! logger.log_mark(None); // will display a mark with no label in the NSLogger viewer
//! logger.log(Level::Info).image(&image_buffer);
//! logger.log(Level::Info).with_domain(Domain::App).message("leaving application");
//! ```
//!
//! ## NOT supported:
//!
//! At the moment there are no plans to add support for the following NSLogger features:
//!
//!  - message blocks
//!  - client disconnects

mod nslogger;

pub use nslogger::{BonjourServiceType, Config, ConnectionMode, Domain, Logger, MessageBuilder};

/// Initializes the global logger with a Logger instance.
///
/// This should be called early in the execution of a Rust program, and the global logger may only
/// be initialized once. Future initialization attempts will return an error.
pub fn init() -> Result<(), log::SetLoggerError> {
    let config @ Config { filter, .. } = Config::parse_env();
    let logger: Logger = config.try_into().unwrap();
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

    use crate::{
        nslogger::{
            BonjourServiceType, Config, Domain, LogMessageType, Logger, SEQUENCE_NB_OFFSET,
        },
        ConnectionMode,
    };

    impl Default for Config {
        fn default() -> Self {
            Self::new(log::LevelFilter::Info, ConnectionMode::default(), true)
        }
    }

    #[test]
    #[serial]
    fn logs_to_file() {
        let tempfile = NamedTempFile::new().expect("temp file");
        let file_path = tempfile.into_temp_path();
        let log: Logger = Config::new(
            log::LevelFilter::Info,
            ConnectionMode::File(file_path.to_path_buf()),
            true,
        )
        .try_into()
        .expect("a valid logger");
        let first_msg = "message logged to file";
        log.log(Level::Warn)
            .with_domain(Domain::App)
            .message(first_msg);
        log.log(Level::Warn)
            .with_domain(Domain::DB)
            .message("other message logged to file");
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
     * NOTE The following tests all rely on the NSLogger desktop application to be running.
     */

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn creates_logger_instance() {
        let log = Logger::default();
        log.log(Level::Warn)
            .with_domain(Domain::App)
            .message("warning msg");
        log.log(Level::Error)
            .with_domain(Domain::DB)
            .message("error msg");
        log.log(Level::Debug)
            .with_domain(Domain::DB)
            .message("debug msg");
        log.log(Level::Debug)
            .with_domain(Domain::Custom("MyCustomDomain".to_string()))
            .message("Tag test");
        log.log(Level::Info).message("Just a simple message");
        std::thread::sleep(Duration::from_secs(2));
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_empty_domain() {
        let log: Logger = Config::default().try_into().unwrap();
        log.log(Level::Warn)
            .with_domain(Domain::Custom("".to_string()))
            .message("no domain should appear");
        std::thread::sleep(Duration::from_secs(2));
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn switches_from_file_to_bonjour() {
        let tempfile = NamedTempFile::new().expect("temp file");
        let mut log: Logger = Config::new(
            log::LevelFilter::Info,
            ConnectionMode::File(tempfile.path().to_path_buf()),
            true,
        )
        .try_into()
        .expect("a valid logger");
        log.log(Level::Warn).message("message logged to file");
        log.set_bonjour_service_mode(BonjourServiceType::Default(false))
            .expect("setting bonjour");
        log.log(Level::Warn).message(format!(
            "previous message should have been logged to file {}",
            tempfile.path().as_os_str().to_string_lossy()
        ));
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn switches_from_bonjour_to_file() {
        let tempfile = NamedTempFile::new().expect("temp file");
        let log: Logger = Config::default().try_into().unwrap();
        log.log(Level::Warn).message(format!(
            "message first logged to Bonjour, next one should appear in {}",
            tempfile.path().as_os_str().to_string_lossy()
        ));
        log.set_file_mode(tempfile.path().to_path_buf())
            .expect("setting file path"); // File extension is constrained!!
        log.log(Level::Warn)
            .message("message shifted from Bonjour to file");
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_mark() {
        let log: Logger = Config::default().try_into().unwrap();
        log.log(Level::Error).message("before mark");
        log.log_mark(Some("this is a mark"));
        log.log(Level::Error).message("after mark");
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_empty_mark() {
        let log: Logger = Config::default().try_into().unwrap();
        log.log(Level::Error).message("before empty mark");
        log.log_mark(None);
        log.log(Level::Error).message("after empty mark");
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_image() {
        use std::{env, fs::File, io::Read};
        let image_path = &env::current_dir().unwrap().join("tests/fixtures/zebra.png");
        let mut file_handle = File::open(image_path).unwrap();
        let mut buffer: Vec<u8> = vec![];

        file_handle.read_to_end(&mut buffer).unwrap();

        let log: Logger = Config::default().try_into().unwrap();
        log.log(Level::Warn).image(&buffer);
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_binary_data() {
        let bytes: [u8; 8] = [0x6c, 0x6f, 0x67, 0x20, 0x74, 0x65, 0x73, 0x74];
        // should read 'log test'

        let log: Logger = Config::default().try_into().unwrap();
        log.log(Level::Warn).data(&bytes);
    }
}
