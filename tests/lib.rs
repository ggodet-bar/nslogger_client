#![feature(rustc_private)]


#[macro_use]
extern crate log ;
extern crate nslogger ;

//#[test]
fn logs_simple_messages() {
    nslogger::init().unwrap() ;

    info!("This is an NSLogger info message") ;
    trace!("This is an NSLogger trace message") ;
    warn!("This is an NSLogger warn message") ;
    error!("This is an NSLogger error message") ;
}

#[test]
fn logs_messages_with_targets() {
    nslogger::init().unwrap() ;

    info!(target:"App", "Should find App domain") ;
    info!(target:"DB", "Should find DB domain") ;
    info!(target:"Custom", "Should create custom domain") ;
}
