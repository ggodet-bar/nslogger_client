#![feature(rustc_private)]


#[macro_use]
extern crate log ;
extern crate nslogger ;

#[test]
fn it_works() {
    nslogger::init().unwrap() ;

    error!("This is an NSLogger info message") ;
    error!("This is an NSLogger info message") ;
    error!("This is an NSLogger info message") ;
    error!("This is an NSLogger info message") ;
    println!("TEST") ;
}

