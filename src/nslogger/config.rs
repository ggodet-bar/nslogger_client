use std::{env, path::PathBuf, str::FromStr};

use crate::nslogger::{BonjourServiceType, ConnectionMode};

/// Logger configuration object.
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

    fn parse_with_getter<F: Fn(&str) -> Option<String>>(getter: F) -> Self {
        let connection_mode = if let Some(val) = getter("NSLOG_FILENAME") {
            PathBuf::from_str(&val)
                .map(ConnectionMode::File)
                .unwrap_or_default()
        } else {
            let use_ssl = getter("NSLOG_USE_SSL").map(|v| v != "0").unwrap_or(true);
            if let Some(val) = getter("NSLOG_HOST") {
                val.split_once(':')
                    .map(|(host, port)| {
                        u16::from_str(port)
                            .map(|p| ConnectionMode::Tcp(host.to_string(), p, use_ssl))
                            .unwrap_or_default()
                    })
                    .unwrap_or_default() // ConnectionMode::Tcp((), (), ())
            } else {
                let service_type = getter("NSLOG_BONJOUR_SERVICE")
                    .map(|s| BonjourServiceType::Custom(s, use_ssl))
                    .unwrap_or(BonjourServiceType::Default(use_ssl));
                ConnectionMode::Bonjour(service_type)
            }
        };
        let flush_messages = getter("NSLOG_FLUSH").map(|v| v == "1").unwrap_or_default();
        let raw_level = getter("NSLOG_LEVEL").unwrap_or("WARN".to_string());
        let level_filter = log::LevelFilter::from_str(&raw_level).unwrap_or(log::LevelFilter::Warn);
        Self {
            filter: level_filter,
            mode: connection_mode,
            flush_messages,
        }
    }

    /// Parses the environment variables into a logger config. Refer to the root crate documentation
    /// for details on the supported variables.
    pub fn parse_env() -> Self {
        Self::parse_with_getter(|key| env::var(key).ok())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn parses_default_env() {
        let env = HashMap::<String, String>::default();
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::default(),
                flush_messages: false
            },
            Config::parse_with_getter(|key| { env.get(key).cloned() })
        )
    }

    #[test]
    fn parses_file_path_from_env() {
        let env: HashMap<_, _> = [(
            "NSLOG_FILENAME".to_string(),
            "/tmp/file_output.log".to_string(),
        )]
        .into_iter()
        .collect();
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::File(PathBuf::from("/tmp/file_output.log")),
                flush_messages: false
            },
            Config::parse_with_getter(|key| { env.get(key).cloned() })
        );
    }

    #[test]
    fn parses_log_level_from_env() {
        let env: HashMap<_, _> = [("NSLOG_LEVEL".to_string(), "INFO".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            Config {
                filter: log::LevelFilter::Info,
                mode: ConnectionMode::default(),
                flush_messages: false
            },
            Config::parse_with_getter(|key| { env.get(key).cloned() })
        );
    }

    #[test]
    fn disables_bonjour_ssl_from_env() {
        let env: HashMap<_, _> = [("NSLOG_USE_SSL".to_string(), "0".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::Bonjour(BonjourServiceType::Default(false)),
                flush_messages: false
            },
            Config::parse_with_getter(|key| { env.get(key).cloned() })
        );
    }

    #[test]
    fn sets_local_host_from_env() {
        let env: HashMap<_, _> = [("NSLOG_HOST".to_string(), "127.0.0.1:50000".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::Tcp("127.0.0.1".to_string(), 50000, true),
                flush_messages: false
            },
            Config::parse_with_getter(|key| { env.get(key).cloned() })
        );
    }

    #[test]
    fn sets_remote_host_from_env() {
        let env: HashMap<_, _> = [("NSLOG_HOST".to_string(), "example.org:50000".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::Tcp("example.org".to_string(), 50000, true),
                flush_messages: false
            },
            Config::parse_with_getter(|key| { env.get(key).cloned() })
        );
    }

    #[test]
    fn sets_message_flushing_from_env() {
        let env: HashMap<_, _> = [("NSLOG_FLUSH".to_string(), "1".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            Config {
                filter: log::LevelFilter::Warn,
                mode: ConnectionMode::default(),
                flush_messages: true
            },
            Config::parse_with_getter(|key| { env.get(key).cloned() })
        );
    }
}
