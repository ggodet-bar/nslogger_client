use std::{io, net::ToSocketAddrs, time::Duration};

use futures::StreamExt;
use log::log;
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use crate::nslogger::{logger_state::Message, DEBUG_LOGGER};

pub enum BonjourServiceStatus {
    ServiceFound(String, String, u16, bool),
    TimedOut,
    Unresolved,
}

pub struct NetworkManager {
    command_rx: mpsc::UnboundedReceiver<BonjourServiceType>,
    message_tx: mpsc::UnboundedSender<Message>,

    bonjour_service: BonjourService,
}

impl NetworkManager {
    pub fn new(
        command_rx: mpsc::UnboundedReceiver<BonjourServiceType>,
        message_tx: mpsc::UnboundedSender<Message>,
    ) -> NetworkManager {
        NetworkManager {
            command_rx,
            message_tx,

            bonjour_service: BonjourService::default(),
        }
    }

    pub async fn run(&mut self) -> io::Result<()> {
        if DEBUG_LOGGER {
            log::info!("starting network manager");
        }

        // TODO Pass a signal to quit the service
        while let Some(service_type) = &self.command_rx.recv().await {
            if DEBUG_LOGGER {
                log::info!("network manager received message");
            }
            let mut is_connected = false;
            while !is_connected {
                match self.bonjour_service.setup_bonjour(service_type).await? {
                    BonjourServiceStatus::ServiceFound(
                        bonjour_service_name,
                        host,
                        port,
                        use_ssl,
                    ) => {
                        log::info!("found Bonjour service {bonjour_service_name}");
                        self.message_tx
                            .send(Message::TryConnectBonjour(
                                bonjour_service_name,
                                host,
                                port,
                                use_ssl,
                            ))
                            .unwrap();
                        is_connected = true;
                    }
                    _ => {
                        if DEBUG_LOGGER {
                            log::info!("couldn't resolve Bonjour. Will retry in a few seconds");
                        }

                        sleep(Duration::from_secs(5)).await
                    }
                }
            }
        }

        if DEBUG_LOGGER {
            log::info!("stopping network manager");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum BonjourServiceType {
    /// Service type
    Custom(String, bool),
    /// Defines whether to use SSL
    Default(bool),
}

#[derive(Default)]
pub struct BonjourService;

impl BonjourService {
    async fn setup_bonjour(
        &mut self,
        service_type: &BonjourServiceType,
    ) -> io::Result<BonjourServiceStatus> {
        if DEBUG_LOGGER {
            log::info!("setting up Bonjour");
        }
        let (service_name, use_ssl) = match service_type {
            BonjourServiceType::Custom(name, use_ssl) => (name.as_str(), *use_ssl),
            BonjourServiceType::Default(use_ssl) if *use_ssl => ("_nslogger-ssl._tcp.", true),
            _ => ("_nslogger._tcp.", false),
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
        let resolve_details = browse_result.resolve().next().await;
        let Some(resolve_details) = resolve_details else {
            return Ok(BonjourServiceStatus::Unresolved);
        };
        let resolve_details = resolve_details?;
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
