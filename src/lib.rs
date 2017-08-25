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

#![feature(thread_id)]

#[macro_use]
extern crate bitflags ;

#[macro_use]
extern crate log ;

#[cfg(test)]
extern crate env_logger ;

extern crate async_dnssd ;
extern crate futures;
extern crate tokio_core;
extern crate openssl ;

extern crate byteorder ;

extern crate chrono ;

#[macro_use]
extern crate cfg_if ;

extern crate sys_info ;
extern crate integer_atomics ;

pub mod nslogger ;

pub use nslogger::Logger ;

/// Initializes the global logger with a Logger instance.
///
/// This should be called early in the execution of a Rust program, and the
/// global logger may only be initialized once. Future initialization
/// attempts will return an error.
pub fn init() -> Result<(), log::SetLoggerError> {
    log::set_logger(|max_log_level| {
        max_log_level.set(log::LogLevelFilter::Info) ;
        let mut logger = Logger::new() ;
        //logger.set_message_flushing(true) ;
        Box::new(logger)
    })
}

#[cfg(test)]
mod tests {
    use nslogger::{ Logger, Domain, Level } ;

    #[test]
    fn connects_via_bonjour_with_ssl() {
        use std::{thread, time} ;
        let mut log = Logger::new() ;
        //log.set_message_flushing(true) ;
        log.logm(Some(Domain::App), Level::Warning, "test1") ;
        log.logm(Some(Domain::App), Level::Warning, "test2") ;
        log.logm(Some(Domain::App), Level::Warning, "test3") ;

        //let ten_millis = time::Duration::from_secs(50);
        //let now = time::Instant::now();

        //thread::sleep(ten_millis);
    }

    #[test]
    fn creates_logger_instance() {
        let log = Logger::new() ;
        log.logm(Some(Domain::App), Level::Warning, "test") ;
        log.logm(Some(Domain::DB), Level::Error, "test1") ;
        log.logm(Some(Domain::DB), Level::Debug, "test2") ;
        log.logm(Some(Domain::DB), Level::Warning, "test") ;
        log.logm(Some(Domain::DB), Level::Error, "test1") ;
        log.logm(Some(Domain::DB), Level::Debug, "test2") ;
        log.logm(Some(Domain::Custom("MyCustomDomain".to_string())), Level::Debug, "Tag test!") ;
        log.log("Just a simple message") ;
    }

    #[test]
    fn logs_empty_domain() {
        let mut log = Logger::new() ;
        log.set_message_flushing(true) ;
        log.logm(Some(Domain::Custom("".to_string())), Level::Warning, "no domain should appear") ;
    }

    #[test]
    fn parses_domain_from_string() {
        use std::str::FromStr ;
        assert_eq!(Domain::App, Domain::from_str("App").unwrap()) ;
        assert_eq!(Domain::DB, Domain::from_str("DB").unwrap()) ;
        assert_eq!(Domain::Custom("CustomTag".to_string()), Domain::from_str("CustomTag").unwrap()) ;

    }

    #[test]
    fn logs_to_file() {
        use std::path::Path ;
        use std::fs ;

        let mut log = Logger::new() ;
        log.set_message_flushing(true) ;
        log.set_log_file_path("/tmp/ns_logger.rawnsloggerdata") ; // File extension is constrained!!
        log.logm(Some(Domain::App), Level::Warning, "message logged to file") ;
        log.logm(Some(Domain::DB), Level::Warning, "other message logged to file") ;

        let file_path = Path::new("/tmp/ns_logger.rawnsloggerdata") ;
        assert!(file_path.exists()) ;


        fs::remove_file(file_path) ;
        assert!(!file_path.exists()) ;
    }

    #[test]
    fn switches_from_file_to_bonjour() {
        let mut log = Logger::new() ;
        log.set_message_flushing(true) ;
        log.set_log_file_path("/tmp/ns_logger.rawnsloggerdata") ; // File extension is constrained!!
        log.logm(Some(Domain::App), Level::Warning, "message logged to file") ;

        log.set_bonjour_service(None, None, false) ;
        //log.set_remote_host("127.0.0.1", 50000, true) ; // SSL Will be on on the desktop client no matter the setting
        log.set_message_flushing(true) ;
        // FIXME Won't work: the change_option method will probably still be running
        log.logm(Some(Domain::App), Level::Warning, "message previously logged to file") ;
    }

    #[test]
    fn switches_from_bonjour_to_file() {
        let mut log = Logger::new() ;
        log.logm(Some(Domain::App), Level::Warning, "message first logged to Bonjour") ;

        log.set_message_flushing(true) ;
        log.set_log_file_path("/tmp/ns_logger.rawnsloggerdata") ; // File extension is constrained!!
        log.logm(Some(Domain::App), Level::Warning, "message shifted from Bonjour to file") ;
    }

    #[test]
    fn flushes_log_messages() {
        // TODO a better approach would probably be to write a small thread that crashes, with a
        // message that has to be passed before the crash?
        let mut log = Logger::new() ;
        log.set_remote_host("127.0.0.1", 50000, true) ; // SSL Will be on on the desktop client no matter the setting
        log.set_message_flushing(true) ;
        log.logm(Some(Domain::App), Level::Warning, "flush test") ;
        log.logm(Some(Domain::DB), Level::Error, "flush test1") ;
        log.logm(Some(Domain::DB), Level::Debug, "flush test2") ;
        log.logm(Some(Domain::DB), Level::Warning, "flush test") ;
        log.logm(Some(Domain::DB), Level::Error, "flush test1") ;
        log.logm(Some(Domain::DB), Level::Debug, "flush test2") ;
    }

    #[test]
    fn logs_mark(){
        let mut log = Logger::new() ;
        log.set_remote_host("127.0.0.1", 50000, true) ;
        log.set_message_flushing(true) ;
        log.logm(Some(Domain::App), Level::Warning, "before mark 1") ;
        log.logm(Some(Domain::DB), Level::Error, "before mark 2") ;
        log.log_mark(Some("this is a mark")) ;
        log.logm(Some(Domain::DB), Level::Debug, "after mark") ;
    }

    #[test]
    fn logs_empty_mark(){
        let mut log = Logger::new() ;
        log.set_remote_host("127.0.0.1", 50000, true) ;
        log.set_message_flushing(true) ;
        log.logm(Some(Domain::App), Level::Warning, "before mark 1") ;
        log.logm(Some(Domain::DB), Level::Error, "before mark 2") ;
        log.log_mark(None) ;
        log.logm(Some(Domain::DB), Level::Debug, "after mark") ;
    }

    #[test]
    fn logs_image() {
        use std::fs::File;
        use std::env ;
        use std::io::Read ;
        let image_path = &env::current_dir().unwrap().join("tests/fixtures/zebra.png") ;
        let mut file_handle = File::open(image_path).unwrap() ;
        let mut buffer:Vec<u8> = vec![] ;

        file_handle.read_to_end(&mut buffer).unwrap() ;

        let mut log = Logger::new() ;
        log.set_remote_host("127.0.0.1", 50000, true) ;
        log.set_message_flushing(true) ;
        log.log_image(None, None, None, None, Level::Warning, &buffer) ;
    }

    #[test]
    fn logs_binary_data() {
        let bytes:[u8;8] = [ 0x6c, 0x6f, 0x67, 0x20, 0x74, 0x65, 0x73, 0x74 ] ;
        // should read 'log test'

        let mut log = Logger::new() ;
        log.set_remote_host("127.0.0.1", 50000, true) ;
        log.set_message_flushing(true) ;
        log.log_data(None, None, None, None, Level::Warning, &bytes) ;
    }

}
