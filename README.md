# nslogger_client

[![Rust](https://github.com/ggodet-bar/NSLoggerClient/actions/workflows/rust.yml/badge.svg)](https://github.com/ggodet-bar/NSLoggerClient/actions/workflows/rust.yml)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A Rust client for the MacOS [NSLogger](https://github.com/fpillet/NSLogger) log viewer (tested on version 1.9.7 of the app).

This implementation started out as a port of the [Java
 client implementation](https://github.com/fpillet/NSLogger/blob/master/Client/Android/client-code/src/com/NSLogger/NSLoggerClient.java),
initially designed for Android, and was later refactored to better match modern Rust coding standards. It may be used either together with the [`log`](https://docs.rs/log) crate, or directly through its API to make use of the advanced NSLogger viewing features (for images, binary data and marks).

## NOT supported:

At the moment there are no plans to add support for the following NSLogger features:

- message blocks
- client disconnects
