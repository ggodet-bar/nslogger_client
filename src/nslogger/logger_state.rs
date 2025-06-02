use std::{
    collections::VecDeque,
    fs::File,
    io,
    io::{BufWriter, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use log::log;
use openssl::{
    self,
    ssl::{SslConnector, SslMethod, SslStream},
};
use tokio::{
    runtime::{Builder, Runtime},
    sync::mpsc,
};

use crate::nslogger::{
    log_message::LogMessage, network_manager, network_manager::BonjourServiceType, Error,
    MessageWorker, Signal, DEBUG_LOGGER,
};

#[derive(Debug)]
pub enum Message {
    TryConnectBonjour(String, String, u16, bool),
    AddLog(LogMessage, Option<Signal>),
    ConnectionModeChange(ConnectionMode),
    Quit,
}

#[derive(Debug, Clone)]
pub enum ConnectionMode {
    Tcp(String, u16, bool),
    Bonjour(BonjourServiceType),
    File(PathBuf),
}

impl Default for ConnectionMode {
    fn default() -> Self {
        Self::Bonjour(BonjourServiceType::Default(true))
    }
}

#[derive(Debug)]
pub enum WriteStreamWrapper {
    Tcp(TcpStream),
    Ssl(SslStream<TcpStream>),
    File(BufWriter<File>),
}

impl std::io::Write for WriteStreamWrapper {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            WriteStreamWrapper::Tcp(stream) => stream.write(buf),
            WriteStreamWrapper::Ssl(stream) => stream.write(buf),
            WriteStreamWrapper::File(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            WriteStreamWrapper::Tcp(stream) => stream.flush(),
            WriteStreamWrapper::Ssl(stream) => stream.flush(),
            WriteStreamWrapper::File(stream) => stream.flush(),
        }
    }
}

pub struct LoggerState {
    pub is_reconnection_scheduled: bool,
    pub is_connecting: bool,
    pub is_connected: bool,
    pub is_client_info_added: bool,
    pub connection_mode: ConnectionMode,
    pub write_stream: Option<WriteStreamWrapper>,
    pub log_messages: VecDeque<(LogMessage, Option<Signal>)>,
    command_tx: mpsc::UnboundedSender<network_manager::BonjourServiceType>,
    runtime: Option<Runtime>,
}

impl LoggerState {
    pub fn new(
        message_tx: mpsc::UnboundedSender<Message>,
        message_rx: mpsc::UnboundedReceiver<Message>,
        ready_signal: Signal,
    ) -> Result<Arc<Mutex<Self>>, Error> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        let state = LoggerState {
            connection_mode: ConnectionMode::default(),
            write_stream: None,
            is_reconnection_scheduled: false,
            is_connecting: false,
            is_connected: false,
            is_client_info_added: false,
            log_messages: VecDeque::new(),
            command_tx,
            runtime: Some(
                Builder::new_multi_thread()
                    .enable_io()
                    .enable_time()
                    .build()?,
            ),
        };
        Self::setup_network_manager(message_tx, command_rx, &state.runtime.as_ref().unwrap())?;
        let state = Arc::new(Mutex::new(state));
        Self::setup_message_worker(state.clone(), message_rx, ready_signal)?;
        Ok(state)
    }

    pub fn process_log_queue(&mut self) -> Result<(), Error> {
        if self.log_messages.is_empty() {
            if DEBUG_LOGGER {
                log::info!("process_log_queue empty");
            }
            return Ok(());
        }

        if DEBUG_LOGGER {
            log::info!("process_log_queue");
        }

        if !self.is_client_info_added {
            self.push_client_info_to_front_of_queue();
        }

        if self.write_stream.is_none() {
            self.setup_connection()?;
        }

        if self.is_connected {
            self.write_messages_to_stream()?;
        }

        if DEBUG_LOGGER {
            log::info!(
                "[{:?}] finished processing log queue",
                thread::current().id()
            );
        }
        Ok(())
    }

    fn setup_network_manager(
        message_tx: mpsc::UnboundedSender<Message>,
        command_rx: mpsc::UnboundedReceiver<BonjourServiceType>,
        runtime: &Runtime,
    ) -> Result<(), Error> {
        runtime.spawn(async move {
            network_manager::NetworkManager::new(command_rx, message_tx)
                .run()
                .await
        });
        Ok(())
    }

    fn setup_message_worker(
        state: Arc<Mutex<LoggerState>>,
        message_rx: mpsc::UnboundedReceiver<Message>,
        ready_signal: Signal,
    ) -> Result<(), Error> {
        let local_state = state.clone();
        let Some(runtime) = &(*local_state.lock().unwrap()).runtime else {
            return Ok(());
        };
        runtime.spawn(async {
            MessageWorker::new(state, message_rx, ready_signal)
                .run()
                .await
        });
        Ok(())
    }

    fn push_client_info_to_front_of_queue(&mut self) {
        if DEBUG_LOGGER {
            log::info!("pushing client info to front of queue");
        }

        self.log_messages
            .push_front((LogMessage::client_info(), None));
        self.is_client_info_added = true;
    }

    pub fn change_options(&mut self, mode: ConnectionMode) -> Result<(), Error> {
        if DEBUG_LOGGER {
            log::info!("changing options: {:?}. Closing/restarting.", mode);
        }
        if self.write_stream.is_some() {
            self.disconnect();
        }
        self.connection_mode = mode;
        self.setup_connection()?;
        Ok(())
    }

    pub fn browse_bonjour_services(&mut self, use_ssl: bool) -> Result<(), Error> {
        self.command_tx
            .send(network_manager::BonjourServiceType::Default(use_ssl))
            .map_err(|_| Error::ChannelNotAvailable)?;

        self.is_connecting = true;

        Ok(())
    }

    pub fn connect_to_remote(
        &mut self,
        host: &str,
        port: u16,
        use_ssl: bool,
    ) -> Result<WriteStreamWrapper, Error> {
        let connect_string = format!("{}:{}", host, port);
        if DEBUG_LOGGER {
            log::info!("connecting to {connect_string}");
        }
        let stream = TcpStream::connect(connect_string)?;
        let stream = if use_ssl {
            if DEBUG_LOGGER {
                log::info!("activating SSL connection");
            }

            // FIXME Rework the whole connection sub-process.
            let mut ssl_connector_builder = SslConnector::builder(SslMethod::tls()).unwrap();

            ssl_connector_builder.set_verify(openssl::ssl::SslVerifyMode::NONE);
            ssl_connector_builder
                .set_verify_callback(openssl::ssl::SslVerifyMode::NONE, |_, _| true);

            let connector = ssl_connector_builder.build();
            // if let WriteStreamWrapper::Tcp(inner_stream) = self.write_stream.take().unwrap() {
            let stream = connector.connect("localhost", stream).unwrap();
            if DEBUG_LOGGER {
                log::info!("opened SSL stream");
            }
            WriteStreamWrapper::Ssl(stream)
        } else {
            WriteStreamWrapper::Tcp(stream)
        };

        self.is_connecting = false;
        self.is_connected = true;

        Ok(stream)
    }

    pub fn disconnect(&mut self) {
        if DEBUG_LOGGER {
            log::info!("disconnect_from_remote()");
        }

        self.is_connected = false;
        self.is_connecting = false;
        self.is_client_info_added = false;

        self.write_stream = None;
    }

    pub fn setup_connection(&mut self) -> Result<(), Error> {
        match self.connection_mode.clone() {
            ConnectionMode::File(path) => {
                let stream = self.create_buffer_write_stream(&path)?;
                self.write_stream = Some(stream);
            }
            ConnectionMode::Tcp(host, port, use_ssl)
                if !(self.is_connecting || self.is_reconnection_scheduled) =>
            {
                let stream = self.connect_to_remote(&host, port, use_ssl)?;
                self.write_stream = Some(stream);
            }
            ConnectionMode::Bonjour(BonjourServiceType::Default(use_ssl))
                if !(self.is_connecting || self.is_reconnection_scheduled) =>
            {
                self.browse_bonjour_services(use_ssl)?;
            }
            _ => {
                // Nothing to do
            }
        };
        Ok(())
    }

    fn try_reconnecting(&mut self) -> Result<(), Error> {
        if self.is_reconnection_scheduled {
            return Ok(());
        }

        if DEBUG_LOGGER {
            log::info!("try_reconnecting");
        }

        self.setup_connection()?;
        Ok(())
    }

    pub fn create_buffer_write_stream(&mut self, path: &Path) -> Result<WriteStreamWrapper, Error> {
        if DEBUG_LOGGER {
            log::info!("creating file buffer stream to {path:?}");
        }

        let file_writer = BufWriter::new(File::create(path)?);
        self.is_connected = true;
        Ok(WriteStreamWrapper::File(file_writer))
    }

    pub fn close_buffer_write_stream(&mut self) -> Result<(), Error> {
        if DEBUG_LOGGER && self.write_stream.is_some() {
            log::info!("closing buffer stream");
        }

        if let Some(mut stream) = self.write_stream.take() {
            stream.flush()?;
        };
        self.is_client_info_added = false;
        Ok(())
    }

    /// Write outstanding messages to the stream
    fn write_messages_to_stream(&mut self) -> Result<(), Error> {
        if DEBUG_LOGGER {
            log::info!(
                "process_log_queue: {} queued messages",
                self.log_messages.len()
            );
        }

        while let Some((mut message, signal)) = self.log_messages.pop_front() {
            {
                if DEBUG_LOGGER {
                    log::info!("processing message {}", &message.sequence_number);
                }

                message.freeze();
                let length = message.data.len();

                let tcp_stream = self.write_stream.as_mut().unwrap();
                if DEBUG_LOGGER {
                    log::info!("writing to {:?} (len: {length})", tcp_stream);
                }
                if let Err(err) = tcp_stream.write_all(&message.data) {
                    self.log_messages.push_front((message, signal));
                    if DEBUG_LOGGER {
                        log::warn!("write to stream failed: {err:?}");
                    }

                    self.disconnect();
                    self.try_reconnecting()?;
                    return Ok(());
                }
                if let Some(signal) = signal {
                    tcp_stream.flush()?;
                    signal.signal();
                }
            }
        }

        Ok(())
    }
}

impl Drop for LoggerState {
    fn drop(&mut self) {
        if DEBUG_LOGGER {
            log::info!("calling drop for logger state");
        }
        self.disconnect();
        self.runtime.take().unwrap().shutdown_background();
    }
}
