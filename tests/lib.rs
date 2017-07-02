#[macro_use]
extern crate log ;
extern crate nslogger ;

use std::sync::{Once, ONCE_INIT};

static START: Once = ONCE_INIT ;

#[test]
fn logs_simple_messages() {
    initialize_logger() ;

    info!("This is an NSLogger info message") ;
    trace!("This is an NSLogger trace message") ;
    warn!("This is an NSLogger warn message") ;
    error!("This is an NSLogger error message") ;
}

#[test]
fn logs_messages_with_targets() {
    initialize_logger() ;

    info!(target:"App", "Should find App domain") ;
    info!(target:"DB", "Should find DB domain") ;
    info!(target:"Custom", "Should create custom domain") ;
}

fn initialize_logger() {
    START.call_once(|| { nslogger::init().unwrap() ; }) ;
}
