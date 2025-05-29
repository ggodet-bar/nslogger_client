use std::{io, net::ToSocketAddrs, time::Duration};

use futures::StreamExt;
use log::log;
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use crate::nslogger::{logger_state::Message, DEBUG_LOGGER};

pub trait BonjourService {
    async fn setup_bonjour<T: BonjourService>(
        &mut self,
        service_name: &str,
    ) -> io::Result<BonjourServiceStatus>;
}

pub enum Command {
    SetupBonjour(String),
    Quit,
}

pub enum BonjourServiceStatus {
    ServiceFound(String, String, u16),
    TimedOut,
    Unresolved,
}

pub struct NetworkManager<T: BonjourService> {
    command_rx: mpsc::UnboundedReceiver<Command>,
    message_tx: mpsc::UnboundedSender<Message>,

    bonjour_service: T,
}

impl<T: BonjourService> NetworkManager<T> {
    pub fn new(
        command_rx: mpsc::UnboundedReceiver<Command>,
        message_tx: mpsc::UnboundedSender<Message>,
        bonjour_service: T,
    ) -> NetworkManager<T> {
        NetworkManager {
            command_rx,
            message_tx,

            bonjour_service,
        }
    }

    pub async fn run(&mut self) -> io::Result<()> {
        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "starting network manager");
        }

        while let Some(message) = &self.command_rx.recv().await {
            if DEBUG_LOGGER {
                log::info!(target:"NSLogger", "network manager received message");
            }

            match message {
                Command::SetupBonjour(service_name) => {
                    let mut is_connected = false;
                    while !is_connected {
                        match self
                            .bonjour_service
                            .setup_bonjour::<T>(&service_name)
                            .await?
                        {
                            BonjourServiceStatus::ServiceFound(
                                bonjour_service_name,
                                host,
                                port,
                            ) => {
                                self.message_tx.send(Message::TryConnectBonjour(
                                    bonjour_service_name,
                                    host,
                                    port,
                                ));
                                is_connected = true;
                            }
                            _ => {
                                if DEBUG_LOGGER {
                                    log::info!(target:"NSLogger", "couldn't resolve Bonjour. Will retry in a few seconds");
                                }

                                sleep(Duration::from_secs(5)).await
                            }
                        }
                    }
                }
                Command::Quit => {
                    if DEBUG_LOGGER {
                        log::info!(target:"NSLogger", "properly exiting the network manager");
                    }

                    break;
                }
            }
        }

        if DEBUG_LOGGER {
            log::info!(target:"NSLogger", "stopping network manager");
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct DefaultBonjourService;

impl BonjourService for DefaultBonjourService {
    async fn setup_bonjour<T: BonjourService>(
        &mut self,
        service_name: &str,
    ) -> io::Result<BonjourServiceStatus> {
        let mut service_browser = async_dnssd::browse(service_name);
        match timeout(Duration::from_secs(5), service_browser.next()).await {
            Ok(Some(Ok(browse_result))) => {
                if DEBUG_LOGGER {
                    log::info!(target:"NSLogger", "Browse result: {:?}", browse_result);
                    log::info!(target:"NSLogger", "Service name: {}", browse_result.service_name);
                }
                let bonjour_service_name = browse_result.service_name.to_string();
                let mut remote_host: Option<String> = None;
                let mut remote_port: Option<u16> = None;
                let resolve_details = browse_result.resolve().next().await;
                let Some(resolve_details) = resolve_details else {
                    return Ok(BonjourServiceStatus::Unresolved);
                };
                let resolve_details = resolve_details?;
                if DEBUG_LOGGER {
                    log::info!(target:"NSLogger", "Service resolution details: {:?}", resolve_details);
                }
                for host_addr in format!("{}:{}", resolve_details.host_target, resolve_details.port)
                    .to_socket_addrs()
                    .unwrap()
                {
                    if host_addr.ip().is_ipv4() {
                        let ip_address = format!("{}", host_addr.ip());
                        if DEBUG_LOGGER {
                            log::info!(target:"NSLogger", "Bonjour host details {:?}", host_addr);
                        }
                        remote_host = Some(ip_address);
                        remote_port = Some(resolve_details.port);
                        break;
                    }
                }

                Ok(BonjourServiceStatus::ServiceFound(
                    bonjour_service_name,
                    remote_host.unwrap(),
                    remote_port.unwrap(),
                ))
            }
            Err(_) => {
                if DEBUG_LOGGER {
                    log::warn!(target:"NSLogger", "Bonjour discovery timed out")
                }

                Ok(BonjourServiceStatus::TimedOut)
            }
            _ => Ok(BonjourServiceStatus::Unresolved),
        }
    }
}
