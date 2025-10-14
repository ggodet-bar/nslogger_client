# nslogger_client

[![Rust](https://github.com/ggodet-bar/NSLoggerClient/actions/workflows/rust.yml/badge.svg)](https://github.com/ggodet-bar/NSLoggerClient/actions/workflows/rust.yml)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A Rust client for the MacOS [NSLogger](https://github.com/fpillet/NSLogger) log viewer (tested on version 1.9.7 of the app).

This implementation started out as a port of the [Java
 client implementation](https://github.com/fpillet/NSLogger/blob/master/Client/Android/client-code/src/com/NSLogger/NSLoggerClient.java),
initially designed for Android, and was later refactored to better match modern Rust coding standards. It may be used either together with the [`log`](https://docs.rs/log) crate, or directly through its API to make use of the advanced NSLogger viewing features (for images, binary data and marks).

## Logger configuration

By default, `nslogger_client` connects to the default application using the default Bonjour
service handle with SSL (`_nslogger-ssl._tcp.`) with no message flushing.

`nslogger_client` may be configured via the following environment variables:

- `NSLOG_LEVEL` - maximum log level, with `TRACE` being the highest value and `ERROR` the lowest
  (cf [`log::Level`]);
- `NSLOG_FLUSH` - whether the logger should wait for the desktop application to send an ack
  signal after each message;
- `NSLOG_FILENAME` - switches the logger to print messages to the given filepath **instead** of
  the destkop application;
- `NSLOG_BONJOUR_SERVICE` - switches the logger to connect to the desktop application using the
  given Bonjour handler;
- `NSLOG_USE_SSL` - switches the logger to connect to the desktop application (whether via
  Bonjour or a direct TCP connection) with or without SSL (`0` disables SSL, any other value
  enables it);
- `NSLOG_HOST` - switches the logger to connect to the desktop application via TCP using the
  `<address>:<port>` format. Domain name resolution is supported;

## Examples

Using the `log` crate facade

```rust
use log::info;

nslogger::init();
info!("this is an NSLogger message");
```

Using the `nslogger_client` API:

```rust
use nslogger::{Logger, Domain};
use log::Level;

let logger = Logger::default();
logger.log(Level::Info).with_domain(Domain::App).message("starting application");
logger.log_mark(None); // will display a mark with no label in the NSLogger viewer
logger.log(Level::Info).with_domain(Domain::App).message("leaving application");
```

## NOT supported:

At the moment there are no plans to add support for the following NSLogger features:

- message blocks
- client disconnects
