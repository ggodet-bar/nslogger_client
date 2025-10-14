use std::{io, net::ToSocketAddrs, time::Duration};

use futures::StreamExt;
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use crate::nslogger::{Message, DEBUG_LOGGER};

const DEFAULT_BONJOUR_SERVICE: &str = "_nslogger._tcp.";
const DEFAULT_BONJOUR_SERVICE_SSL: &str = "_nslogger-ssl._tcp.";
const RETRY_TIMEOUT: Duration = Duration::from_secs(5);

pub enum BonjourServiceStatus {
    ServiceFound(String, String, u16, bool),
    TimedOut,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BonjourServiceType {
    /// Service type, and whether to use SSL
    Custom(String, bool),
    /// Defines whether to use SSL
    Default(bool),
}

pub struct NetworkManager {
    command_rx: mpsc::UnboundedReceiver<BonjourServiceType>,
    message_tx: mpsc::UnboundedSender<Message>,
}

impl NetworkManager {
    pub fn new(
        command_rx: mpsc::UnboundedReceiver<BonjourServiceType>,
        message_tx: mpsc::UnboundedSender<Message>,
    ) -> NetworkManager {
        NetworkManager {
            command_rx,
            message_tx,
        }
    }

    pub async fn run(&mut self) -> io::Result<()> {
        if DEBUG_LOGGER {
            log::info!("starting network manager");
        }

        while let Some(service_type) = &self.command_rx.recv().await {
            if DEBUG_LOGGER {
                log::info!("network manager received message");
            }
            let mut is_connected = false;
            while !is_connected {
                match self.setup_bonjour(service_type).await? {
                    BonjourServiceStatus::ServiceFound(
                        bonjour_service_name,
                        host,
                        port,
                        use_ssl,
                    ) => {
                        if DEBUG_LOGGER {
                            log::info!("found Bonjour service {bonjour_service_name}");
                        }
                        self.message_tx
                            .send(Message::ConnectToBonjourService(host, port, use_ssl))
                            .unwrap();
                        is_connected = true;
                    }
                    _ => {
                        if DEBUG_LOGGER {
                            log::info!("couldn't resolve Bonjour. Will retry in a few seconds");
                        }

                        sleep(RETRY_TIMEOUT).await
                    }
                }
            }
        }

        if DEBUG_LOGGER {
            log::info!("exiting network manager");
        }
        Ok(())
    }

    async fn setup_bonjour(
        &mut self,
        service_type: &BonjourServiceType,
    ) -> io::Result<BonjourServiceStatus> {
        if DEBUG_LOGGER {
            log::info!("setting up Bonjour");
        }
        let (service_name, use_ssl) = match service_type {
            BonjourServiceType::Custom(name, use_ssl) => (name.as_str(), *use_ssl),
            BonjourServiceType::Default(use_ssl) if *use_ssl => (DEFAULT_BONJOUR_SERVICE_SSL, true),
            _ => (DEFAULT_BONJOUR_SERVICE, false),
        };
        let mut service_browser = async_dnssd::browse(service_name);
        let browse_result = match timeout(Duration::from_secs(5), service_browser.next()).await {
            Ok(Some(Ok(browse_result))) => browse_result,
            Err(_) => {
                if DEBUG_LOGGER {
                    log::warn!("Bonjour discovery timed out")
                }
                return Ok(BonjourServiceStatus::TimedOut);
            }
            _ => return Ok(BonjourServiceStatus::Unresolved),
        };
        if DEBUG_LOGGER {
            log::info!("browse result: {:?}", browse_result);
        }
        let bonjour_service_name = browse_result.service_name.to_string();
        let resolve_details = browse_result.resolve().next().await.transpose()?;
        let Some(resolve_details) = resolve_details else {
            return Ok(BonjourServiceStatus::Unresolved);
        };
        if DEBUG_LOGGER {
            log::info!("service resolution details: {:?}", resolve_details);
        }
        let Some(host_addr) = format!("{}:{}", resolve_details.host_target, resolve_details.port)
            .to_socket_addrs()?
            .next()
        else {
            return Ok(BonjourServiceStatus::Unresolved);
        };
        let ip_address = host_addr.ip().to_string();
        if DEBUG_LOGGER {
            log::info!("Bonjour host details {host_addr:?}");
        }

        Ok(BonjourServiceStatus::ServiceFound(
            bonjour_service_name,
            ip_address,
            resolve_details.port,
            use_ssl,
        ))
    }
}
