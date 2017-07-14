A client for NSLogger.

The `Logger` is essentially a port of the [Java
implementation](https://github.com/fpillet/NSLogger/blob/master/Client/Android/client-code/src/com/NSLogger/NSLoggerClient.java),
initially designed for Android. Compatible with `log` (obviously without the mark, data and
image logging features). Tested on version 1.8.2 of the MacOS NSLogger server.

TODO:

- opt-out of the networking features (esp. openssl)
- builder pattern for logger initialization
- possibly some optimizations.
