Currently missing:

- Dump to file
- option changes
- client info
- wrapper macros
- integration tests.
- some kind of counter that will record network acks and successful file writes so we can better check the logger's behavior
- builder api for setting up the logger.
- cargo features defining a core logger functionality (which matches the basic log macros) and additional features for enabling networking (Bonjour etc.) and non-standard features (log mark, log images, log data)
