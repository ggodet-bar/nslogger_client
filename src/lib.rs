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
//! let log = Logger::default();
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

mod nslogger;

use nslogger::Config;
pub use nslogger::{BonjourServiceType, ConnectionMode, Domain, Logger};

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

    use crate::nslogger::{BonjourServiceType, Domain, LogMessageType, Logger, SEQUENCE_NB_OFFSET};

    #[test]
    #[serial]
    fn logs_to_file() {
        let tempfile = NamedTempFile::new().expect("temp file");
        let file_path = tempfile.into_temp_path();
        let mut log = Logger::default();
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
     * NOTE The following tests all rely on NSLogger to be running.
     */

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn creates_logger_instance() {
        let log = Logger::default();
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
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn connects_via_bonjour_with_ssl() {
        let mut log = Logger::default();
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warn, "test1");
        log.logm(Some(Domain::App), Level::Warn, "test2");
        log.logm(Some(Domain::App), Level::Warn, "test3");
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_empty_domain() {
        let mut log = Logger::default();
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
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn switches_from_file_to_bonjour() {
        let tempfile = NamedTempFile::new().expect("temp file");

        let mut log = Logger::default();
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
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn switches_from_bonjour_to_file() {
        let tempfile = NamedTempFile::new().expect("temp file");
        let mut log = Logger::default();
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
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn flushes_log_messages() {
        let mut log = Logger::default();
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
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_mark() {
        let mut log = Logger::default();
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warn, "before mark 1");
        log.logm(Some(Domain::DB), Level::Error, "before mark 2");
        log.log_mark(Some("this is a mark"));
        log.logm(Some(Domain::DB), Level::Debug, "after mark");
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_empty_mark() {
        let mut log = Logger::default();
        log.set_message_flushing(true);
        log.logm(Some(Domain::App), Level::Warn, "before mark 1");
        log.logm(Some(Domain::DB), Level::Error, "before mark 2");
        log.log_mark(None);
        log.logm(Some(Domain::DB), Level::Debug, "after mark");
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

        let mut log = Logger::default();
        log.set_message_flushing(true);
        log.log_image(None, None, None, None, Level::Warn, &buffer);
    }

    #[test]
    #[serial]
    #[cfg_attr(not(feature = "desktop-integration"), ignore)]
    fn logs_binary_data() {
        let bytes: [u8; 8] = [0x6c, 0x6f, 0x67, 0x20, 0x74, 0x65, 0x73, 0x74];
        // should read 'log test'

        let mut log = Logger::default();
        log.set_message_flushing(true);
        log.log_data(None, None, None, None, Level::Warn, &bytes);
    }
}
