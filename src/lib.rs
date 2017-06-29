#![feature(rustc_private)]
#![feature(integer_atomics)]
#![feature(ip)]
#![feature(lookup_host)]
#![feature(thread_id)]

extern crate mio ;

#[macro_use]
extern crate bitflags ;

#[macro_use]
extern crate log ;
extern crate env_logger ;

extern crate async_dnssd ;
extern crate futures;
extern crate tokio_core;
extern crate openssl ;

extern crate byteorder ;

extern crate chrono ;

#[macro_use]
extern crate enum_primitive ;

pub mod nslogger ;

use nslogger::Logger ;

/// Initializes the global logger with a Logger instance.
///
/// This should be called early in the execution of a Rust program, and the
/// global logger may only be initialized once. Future initialization
/// attempts will return an error.
pub fn init() -> Result<(), log::SetLoggerError> {
    log::set_logger(|max_log_level| {
        max_log_level.set(log::LogLevelFilter::Info) ;
        Box::new(Logger::new())
    })
}

#[cfg(test)]
mod tests {
    use nslogger::{ Logger, Domain, Level } ;
    use async_dnssd::{Interface, BrowseResult} ;
    use tokio_core::reactor::{Core,Timeout} ;
    use futures::Async ;
    use futures::Stream ;
    use async_dnssd ;
    use std::net ;
    use std::net::ToSocketAddrs ;
    use std::time::Duration ;
    use futures::Future ;
    use futures::future::Either ;
    use futures::IntoFuture ;
    use std::thread ;

    #[test]
    fn connects_via_bonjour_with_ssl() {
        let mut log = Logger::new() ;
        log.set_message_flushing(true) ;
        log.log_b(Some(Domain::App), Level::Warning, "test") ;
    }

    #[test]
    fn creates_logger_instance() {
        let mut log = Logger::new() ;
        log.log_b(Some(Domain::App), Level::Warning, "test") ;
        log.log_b(Some(Domain::DB), Level::Error, "test1") ;
        log.log_b(Some(Domain::DB), Level::Debug, "test2") ;
        log.log_b(Some(Domain::DB), Level::Warning, "test") ;
        log.log_b(Some(Domain::DB), Level::Error, "test1") ;
        log.log_b(Some(Domain::DB), Level::Debug, "test2") ;
        log.log_b(Some(Domain::Custom("MyCustomDomain".to_string())), Level::Debug, "Tag test!") ;
        log.log_c("Just a simple message") ;
        //thread::sleep(Duration::from_secs(100)) ;
    }

    #[test]
    fn logs_empty_domain() {
        let mut log = Logger::new() ;
        log.set_message_flushing(true) ;
        log.log_b(Some(Domain::Custom("".to_string())), Level::Warning, "no domain should appear") ;
    }

    #[test]
    fn parses_domain_from_string() {
        use std::str::FromStr ;
        assert_eq!(Domain::App, Domain::from_str("App").unwrap()) ;
        assert_eq!(Domain::DB, Domain::from_str("DB").unwrap()) ;
        assert_eq!(Domain::Custom("CustomTag".to_string()), Domain::from_str("CustomTag").unwrap()) ;

    }

    #[test]
    fn flushes_log_messages() {
        // TODO a better approach would probably be to write a small thread that crashes, with a
        // message that has to be passed before the crash?
        let mut log = Logger::new() ;
        log.set_remote_host("192.168.0.8", 50000, true) ; // SSL Will be on on the desktop client no matter the setting
        log.set_message_flushing(true) ;
        log.log_b(Some(Domain::App), Level::Warning, "flush test") ;
        log.log_b(Some(Domain::DB), Level::Error, "flush test1") ;
        log.log_b(Some(Domain::DB), Level::Debug, "flush test2") ;
        log.log_b(Some(Domain::DB), Level::Warning, "flush test") ;
        log.log_b(Some(Domain::DB), Level::Error, "flush test1") ;
        log.log_b(Some(Domain::DB), Level::Debug, "flush test2") ;
    }

    #[test]
    fn logs_mark(){
        let mut log = Logger::new() ;
        log.set_remote_host("192.168.0.8", 50000, true) ;
        log.set_message_flushing(true) ;
        log.log_b(Some(Domain::App), Level::Warning, "before mark 1") ;
        log.log_b(Some(Domain::DB), Level::Error, "before mark 2") ;
        log.log_mark(Some("this is a mark")) ;
        log.log_b(Some(Domain::DB), Level::Debug, "after mark") ;
    }

    #[test]
    fn logs_empty_mark(){
        let mut log = Logger::new() ;
        log.set_remote_host("192.168.0.8", 50000, true) ;
        log.set_message_flushing(true) ;
        log.log_b(Some(Domain::App), Level::Warning, "before mark 1") ;
        log.log_b(Some(Domain::DB), Level::Error, "before mark 2") ;
        log.log_mark(None) ;
        log.log_b(Some(Domain::DB), Level::Debug, "after mark") ;
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
        log.set_remote_host("192.168.0.8", 50000, true) ;
        log.set_message_flushing(true) ;
        log.log_image(None, None, None, None, Level::Warning, &buffer) ;
    }

    #[test]
    fn logs_binary_data() {
        let bytes:[u8;8] = [ 0x6c, 0x6f, 0x67, 0x20, 0x74, 0x65, 0x73, 0x74 ] ;
        // should read 'log test'

        let mut log = Logger::new() ;
        log.set_remote_host("192.168.0.8", 50000, true) ;
        log.set_message_flushing(true) ;
        log.log_data(None, None, None, None, Level::Warning, &bytes) ;
    }
}
