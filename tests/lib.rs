use std::sync::Once;

#[macro_use]
extern crate log;

static START: Once = Once::new();

fn initialize_logger() {
    START.call_once(|| {
        nslogger::init().unwrap();
    });
}

/*
 * NOTE The following tests all rely on NSLogger to be running.
 */

#[test]
#[cfg_attr(not(feature = "desktop-integration"), ignore)]
fn logs_simple_messages() {
    initialize_logger();

    info!("This is an NSLogger info message");
    trace!("This is an NSLogger trace message");
    warn!("This is an NSLogger warn message");
    error!("This is an NSLogger error message");
}

#[test]
#[cfg_attr(not(feature = "desktop-integration"), ignore)]
fn logs_messages_with_targets() {
    initialize_logger();

    info!(target:"App", "Should find App domain");
    info!(target:"DB", "Should find DB domain");
    info!(target:"Custom", "Should create custom domain");
}

#[test]
#[cfg_attr(not(feature = "desktop-integration"), ignore)]
fn logs_messages_starting_from_different_threads() {
    use std::{
        sync::{Arc, Barrier},
        thread::spawn,
    };

    initialize_logger();

    let thread_count = 100;

    let mut handles = Vec::with_capacity(thread_count);
    let barrier = Arc::new(Barrier::new(thread_count));
    for i in 0..thread_count {
        let c = barrier.clone();
        handles.push(spawn(move || {
            c.wait();
            // last call to wait will release all threads

            warn!("Warn message 1 from thread-{}", i);
            warn!("Warn message 2 from thread-{}", i);
            warn!("Warn message 3 from thread-{}", i);
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    use std::{thread, time};

    let ten_millis = time::Duration::from_secs(2);
    thread::sleep(ten_millis);
}
