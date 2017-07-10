Currently missing:

- integration tests. find a way to cover disconnections/reconnections.
- some kind of counter that will record network acks and successful file writes so we can better check the logger's behavior
- builder api for setting up the logger.
- cargo features defining a core logger functionality (which matches the basic log macros) and additional features for enabling networking (Bonjour etc.) and non-standard features (log mark, log images, log data)
- formatting should NOT occur on the main thread
- Convert `write_all` to `write`, which should provide a better control in case of disconnections.
- MessageWorker & LoggerState will probably be merged
- Remove openssl dependency using https://docs.rs/rustls/0.9.0/rustls/
