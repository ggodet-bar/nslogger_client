use std::{io, net::ToSocketAddrs, time::Duration};

use futures::StreamExt;
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use crate::nslogger::{Error, Message};

const DEFAULT_BONJOUR_SERVICE: &str = "_nslogger._tcp.";
const DEFAULT_BONJOUR_SERVICE_SSL: &str = "_nslogger-ssl._tcp.";
const RETRY_TIMEOUT: Duration = Duration::from_secs(5);

pub enum BonjourServiceStatus {
    ServiceFound(String, String, bool),
    TimedOut,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BonjourServiceType {
    /// Custom Bonjour service handle, and whether to use SSL
    Custom(String, bool),
    /// Defines whether to use SSL
    Default(bool),
}

impl BonjourServiceType {
    pub fn requires_ssl(&self) -> bool {
        match self {
            Self::Custom(_, use_ssl) | Self::Default(use_ssl) => *use_ssl,
        }
    }
}

/// Handles Bonjour service discovery, based on the commands it receives.
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

    pub async fn run(&mut self) -> Result<(), Error> {
        #[cfg(test)]
        log::info!("starting network manager");

        while let Some(service_type) = &self.command_rx.recv().await {
            #[cfg(test)]
            log::info!("network manager received message");
            let mut is_connected = false;
            while !is_connected {
                match self.setup_bonjour(service_type).await? {
                    BonjourServiceStatus::ServiceFound(_bonjour_service_name, host, use_ssl) => {
                        #[cfg(test)]
                        log::info!("found Bonjour service {_bonjour_service_name}");
                        self.message_tx
                            .send(Message::ConnectToBonjourService(host, use_ssl))
                            .map_err(|_| Error::ChannelNotAvailable)?;
                        is_connected = true;
                    }
                    _ => {
                        #[cfg(test)]
                        log::info!("couldn't resolve Bonjour. Will retry in a few seconds");

                        sleep(RETRY_TIMEOUT).await
                    }
                }
            }
        }

        #[cfg(test)]
        log::info!("exiting network manager");
        Ok(())
    }

    async fn setup_bonjour(
        &mut self,
        service_type: &BonjourServiceType,
    ) -> io::Result<BonjourServiceStatus> {
        #[cfg(test)]
        log::info!("setting up Bonjour");
        let (service_name, use_ssl) = match service_type {
            BonjourServiceType::Custom(name, use_ssl) => (name.as_str(), *use_ssl),
            BonjourServiceType::Default(use_ssl) if *use_ssl => (DEFAULT_BONJOUR_SERVICE_SSL, true),
            _ => (DEFAULT_BONJOUR_SERVICE, false),
        };
        let mut service_browser = async_dnssd::browse(service_name);
        let browse_result = match timeout(Duration::from_secs(5), service_browser.next()).await {
            Ok(Some(Ok(browse_result))) => browse_result,
            Err(_) => {
                #[cfg(test)]
                log::warn!("Bonjour discovery timed out");
                return Ok(BonjourServiceStatus::TimedOut);
            }
            _ => return Ok(BonjourServiceStatus::Unresolved),
        };
        #[cfg(test)]
        log::info!("browse result: {:?}", browse_result);
        let bonjour_service_name = browse_result.service_name.to_string();
        let resolve_details = browse_result.resolve().next().await.transpose()?;
        let Some(resolve_details) = resolve_details else {
            return Ok(BonjourServiceStatus::Unresolved);
        };
        #[cfg(test)]
        log::info!("service resolution details: {:?}", resolve_details);
        let Some(host_addr) = format!("{}:{}", resolve_details.host_target, resolve_details.port)
            .to_socket_addrs()?
            .next()
        else {
            return Ok(BonjourServiceStatus::Unresolved);
        };
        #[cfg(test)]
        log::info!("Bonjour host details {host_addr:?}");

        Ok(BonjourServiceStatus::ServiceFound(
            bonjour_service_name,
            host_addr.to_string(),
            use_ssl,
        ))
    }
}
