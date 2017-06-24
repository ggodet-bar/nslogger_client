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

#[macro_use]
extern crate serde_derive ;
extern crate bincode ;

extern crate async_dnssd ;
extern crate futures;
extern crate tokio_core;
extern crate openssl ;

extern crate byteorder ;

pub mod nslogger ;

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
    fn creates_logger_instance() {
        let mut log = Logger::new() ;
        thread::sleep(Duration::from_secs(5)) ;
        //log.set_remote_host("192.168.0.8", 50000, true) ; // SSL Will be on on the desktop client no matter the setting
        log.log_b(Domain::App, Level::Warning, "test") ;
        log.log_b(Domain::DB, Level::Error, "test1") ;
        log.log_b(Domain::DB, Level::Debug, "test2") ;
        log.log_b(Domain::DB, Level::Warning, "test") ;
        log.log_b(Domain::DB, Level::Error, "test1") ;
        log.log_b(Domain::DB, Level::Debug, "test2") ;
        thread::sleep(Duration::from_secs(100)) ;
        assert_eq!(1,2) ;
    }


    //#[test]
    fn it_works() {

        thread::spawn(move || {
        let mut core = Core::new().unwrap() ;
        let handle = core.handle() ;
        let mut listener = async_dnssd::browse(Interface::Any, "_nslogger-ssl._tcp", None, &handle).unwrap() ;

        println!("Running server") ;
        core.run(listener.for_each( |browse_result| {
            println!("{:?}", browse_result) ;


            let mut resolve = browse_result.resolve(&handle).unwrap() ;
            resolve.and_then( |v| { println!("{:?}", v) ; Ok( () ) }) ;
            //core.run(resolve.and_then( |v| {
                //println!("{:?}", v) ;
            //})) ;
            //core.run(resolve.into_future()).for_each( |resolve_result| {
                //let mut resolve = resolve_result ;
                //println!("{:?}", resolve) ;
            //}) ;
            //resolve.for_each( |resolve_result| {
                //println!("{:?}", resolve_result) ;
            //}) ;

            Ok( () )
        })) ;
        }) ;
        thread::sleep(Duration::from_secs(100)) ;
        assert_eq!(1, 2) ;
    }
}
