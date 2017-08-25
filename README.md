# A client for NSLogger.

[![Build Status](https://travis-ci.org/ggodet-bar/NSLoggerClient.svg?branch=master)](https://travis-ci.org/ggodet-bar/NSLoggerClient)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

The `Logger` is essentially a port of the [Java
implementation](https://github.com/fpillet/NSLogger/blob/master/Client/Android/client-code/src/com/NSLogger/NSLoggerClient.java),
initially designed for Android. Compatible with `log` (obviously without the mark, data and
image logging features). Tested on version 1.8.2 of the MacOS NSLogger server.

## TODO:

- opt-out of the networking features (esp. openssl)
- builder pattern for logger initialization
- possibly some optimizations.
- Typical integration test issue: when we're not waiting for message flushes, the test completes before the bonjour search had sufficient time to complete, therefore all the messages that should have been logged are lost. In this case, the logger should fetch the buffer file's content.
