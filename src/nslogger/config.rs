use std::{env, path::PathBuf, str::FromStr};

use crate::nslogger::{BonjourServiceType, ConnectionMode};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Max logging level.
    pub filter: log::LevelFilter,

    /// Type of connection to the desktop app (or log file path).
    pub mode: ConnectionMode,

    /// Whether the logger should wait for each message to be handled by the log worker before
    /// returnign from the log calls.
    pub flush_messages: bool,
}

impl Config {
    pub fn new(filter: log::LevelFilter, mode: ConnectionMode, flush_messages: bool) -> Self {
        Self {
            filter,
            mode,
            flush_messages,
        }
    }

    /// Parses the environment variables into a logger config.
    pub fn parse_env() -> Self {
        let connection_mode = if let Ok(val) = env::var("NSLOG_FILENAME") {
            PathBuf::from_str(&val)
                .map(ConnectionMode::File)
                .unwrap_or_default()
        } else {
            let use_ssl = env::var("NSLOG_USE_SSL").map(|v| v != "0").unwrap_or(true);
            if let Ok(val) = env::var("NSLOG_HOST") {
                val.split_once(':')
                    .map(|(host, port)| {
                        u16::from_str(port)
                            .map(|p| ConnectionMode::Tcp(host.to_string(), p, use_ssl))
                            .unwrap_or_default()
                    })
                    .unwrap_or_default() // ConnectionMode::Tcp((), (), ())
            } else {
                let service_type = env::var("NSLOG_BONJOUR_SERVICE")
                    .map(|s| BonjourServiceType::Custom(s, use_ssl))
                    .unwrap_or(BonjourServiceType::Default(use_ssl));
                ConnectionMode::Bonjour(service_type)
            }
        };
        let flush_messages = env::var("NSLOG_FLUSH")
            .map(|v| v == "1")
            .unwrap_or_default();
        let raw_level = env::var("NSLOG_LEVEL").unwrap_or("WARN".to_string());
        let level_filter = log::LevelFilter::from_str(&raw_level).unwrap_or(log::LevelFilter::Warn);
        Self {
            filter: level_filter,
            mode: connection_mode,
            flush_messages,
        }
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial]
    fn parses_default_env() {
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::default(),
                flush_messages: false
            },
            Config::parse_env()
        )
    }

    #[test]
    #[serial]
    fn parses_file_path_from_env() {
        unsafe {
            env::set_var("NSLOG_FILENAME", "/tmp/file_output.log");
        }
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::File(PathBuf::from("/tmp/file_output.log")),
                flush_messages: false
            },
            Config::parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_FILENAME");
        }
    }

    #[test]
    #[serial]
    fn parses_log_level_from_env() {
        unsafe {
            env::set_var("NSLOG_LEVEL", "INFO");
        }
        assert_eq!(
            Config {
                filter: log::LevelFilter::Info,
                mode: ConnectionMode::default(),
                flush_messages: false
            },
            Config::parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_LEVEL");
        }
    }

    #[test]
    #[serial]
    fn disables_bonjour_ssl_from_env() {
        unsafe {
            env::set_var("NSLOG_USE_SSL", "0");
        }
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::Bonjour(BonjourServiceType::Default(false)),
                flush_messages: false
            },
            Config::parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_USE_SSL");
        }
    }

    #[test]
    #[serial]
    fn sets_remote_host_from_env() {
        unsafe {
            env::set_var("NSLOG_HOST", "127.0.0.1:50000");
        }
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::Tcp("127.0.0.1".to_string(), 50000, true),
                flush_messages: false
            },
            Config::parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_HOST");
        }
    }

    #[test]
    #[serial]
    fn sets_message_flushing_from_env() {
        unsafe {
            env::set_var("NSLOG_FLUSH", "1");
        }
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::default(),
                flush_messages: true
            },
            Config::parse_env()
        );
        unsafe {
            env::remove_var("NSLOG_FLUSH");
        }
    }
}
