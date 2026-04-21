use std::{
    collections::VecDeque,
    io,
    net::ToSocketAddrs,
    path::{Path, PathBuf},
    sync::Arc,
};

use rustls::{pki_types::ServerName, ClientConfig, RootCertStore};
use tokio::{
    fs::File,
    io::{AsyncWriteExt, BufWriter},
    net::TcpStream,
    sync::mpsc,
};
use tokio_rustls::{client::TlsStream, TlsConnector};
use webpki_roots::TLS_SERVER_ROOTS;

use crate::nslogger::{
    log_message::LogMessage, network_manager, network_manager::BonjourServiceType, Error, Signal,
};

#[derive(Debug)]
pub enum Message {
    ConnectToBonjourService(String, bool),
    AddLog(LogMessage, Option<Signal>),
    SwitchConnection(ConnectionMode),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionMode {
    Tcp(String, u16, bool),
    Bonjour(BonjourServiceType),
    File(PathBuf),
}

impl Default for ConnectionMode {
    fn default() -> Self {
        Self::Bonjour(BonjourServiceType::Default(false))
    }
}

impl ConnectionMode {
    pub fn requires_ssl(&self) -> bool {
        match self {
            Self::Tcp(_, _, use_ssl) => *use_ssl,
            Self::Bonjour(service_type) => service_type.requires_ssl(),
            Self::File(_) => false,
        }
    }
}

#[derive(Debug)]
pub enum WriteStreamWrapper {
    Tcp(TcpStream),
    Ssl(TlsStream<TcpStream>),
    File(BufWriter<File>),
}

impl WriteStreamWrapper {
    async fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            WriteStreamWrapper::Tcp(stream) => stream.write_all(buf).await,
            WriteStreamWrapper::Ssl(stream) => stream.write_all(buf).await,
            WriteStreamWrapper::File(stream) => stream.write_all(buf).await,
        }
    }

    async fn flush(&mut self) -> io::Result<()> {
        match self {
            WriteStreamWrapper::Tcp(stream) => stream.flush().await,
            WriteStreamWrapper::Ssl(stream) => stream.flush().await,
            WriteStreamWrapper::File(stream) => stream.flush().await,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Ready,
}

pub struct LogWorker {
    message_rx: mpsc::UnboundedReceiver<Message>,
    ready_signal: Signal,
    sequence_generator: u32,
    connection_state: ConnectionState,
    /// Rustls client config that allows the runtime to spawn new secure connections.
    ssl_config: Arc<ClientConfig>,
    pub connection_mode: ConnectionMode,
    pub write_stream: Option<WriteStreamWrapper>,
    pub log_messages: VecDeque<(LogMessage, Option<Signal>)>,
    command_tx: mpsc::UnboundedSender<network_manager::BonjourServiceType>,
}

impl LogWorker {
    pub fn new(
        command_tx: mpsc::UnboundedSender<network_manager::BonjourServiceType>,
        message_rx: mpsc::UnboundedReceiver<Message>,
        ready_signal: Signal,
    ) -> Self {
        let root_store = RootCertStore::from_iter(TLS_SERVER_ROOTS.iter().cloned());
        let ssl_config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let ssl_config = Arc::new(ssl_config);
        /*
         * NOTE the worker won't process the client info message, hence the very first message is
         * skipped.
         */
        Self {
            sequence_generator: 1,
            ssl_config,
            message_rx,
            ready_signal,
            connection_mode: ConnectionMode::default(),
            write_stream: None,
            connection_state: ConnectionState::default(),
            log_messages: VecDeque::new(),
            command_tx,
        }
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        #[cfg(test)]
        log::info!("logging thread starting up");
        /*
         * Initial setup according to current parameters.
         */
        self.setup_connection().await?;
        #[cfg(test)]
        log::info!("starting log event loop");
        self.run_loop().await?;
        #[cfg(test)]
        log::info!("stopped log event loop");

        Ok(())
    }

    /// Returns a strictly monotonic sequence number.
    fn generate_sequence_nb(&mut self) -> u32 {
        let nb = self.sequence_generator;
        self.sequence_generator += 1;
        nb
    }

    async fn handle_message(&mut self, message: Message) -> Result<(), Error> {
        #[cfg(test)]
        log::info!("received message");
        match message {
            Message::AddLog(mut message, signal) => {
                /*
                 * Sequence number is set on receiving the message in the handler to
                 * guarantee a strictly monotonic sequence.
                 */
                let sequence_number = self.generate_sequence_nb();
                message.set_sequence_number(sequence_number);
                #[cfg(test)]
                log::info!("adding log {} to the queue", sequence_number);
                self.log_messages.push_back((message, signal));
                self.process_log_queue().await?;
            }
            Message::SwitchConnection(new_mode) => {
                self.change_connection_mode(new_mode).await?;
            }
            Message::ConnectToBonjourService(host, use_ssl) => {
                let stream =
                    Self::connect_to_remote(&host, use_ssl, self.ssl_config.clone()).await?;
                self.connection_state = ConnectionState::Connected;
                self.write_stream = Some(stream);
                self.process_log_queue().await?;
            }
        }
        Ok(())
    }

    pub async fn run_loop(&mut self) -> Result<(), Error> {
        /*
         * We are ready to run. Unpark the waiting threads now
         */
        self.ready_signal.signal();

        while let Some(message) = self.message_rx.recv().await {
            self.handle_message(message).await?;
        }

        self.close_buffer_write_stream().await?;

        Ok(())
    }

    fn push_client_info_to_front_of_queue(&mut self) {
        #[cfg(test)]
        log::info!("pushing client info to front of queue");
        self.log_messages
            .push_front((LogMessage::client_info(), None));
        self.connection_state = ConnectionState::Ready;
    }

    pub async fn change_connection_mode(&mut self, mode: ConnectionMode) -> Result<(), Error> {
        #[cfg(test)]
        log::info!("changing options: {:?}. Closing/restarting.", mode);
        if self.write_stream.is_some() {
            self.disconnect();
        }
        self.connection_mode = mode;
        self.setup_connection().await?;
        Ok(())
    }

    pub fn browse_bonjour_services(&mut self, use_ssl: bool) -> Result<(), Error> {
        self.command_tx
            .send(network_manager::BonjourServiceType::Default(use_ssl))
            .map_err(|_| Error::ChannelNotAvailable)?;

        self.connection_state = ConnectionState::Connecting;

        Ok(())
    }

    pub async fn connect_to_remote(
        host: &str,
        use_ssl: bool,
        ssl_config: Arc<ClientConfig>,
    ) -> Result<WriteStreamWrapper, Error> {
        let ip_address = host.to_socket_addrs()?.next().ok_or(Error::InvalidHost)?;

        #[cfg(test)]
        log::info!("connecting to {ip_address}");
        let stream = TcpStream::connect(&ip_address).await?;
        let stream = if use_ssl {
            #[cfg(test)]
            log::info!("activating SSL connection");
            let connector = TlsConnector::from(ssl_config);
            let server_name = ServerName::try_from(host.to_owned())?;
            let stream = connector.connect(server_name, stream).await?;
            #[cfg(test)]
            log::info!("opened SSL stream");
            WriteStreamWrapper::Ssl(stream)
        } else {
            WriteStreamWrapper::Tcp(stream)
        };

        Ok(stream)
    }

    pub fn disconnect(&mut self) {
        #[cfg(test)]
        log::info!("disconnect_from_remote()");

        self.connection_state = ConnectionState::Disconnected;
        self.write_stream = None;
        self.sequence_generator = 1;
    }

    pub async fn setup_connection(&mut self) -> Result<(), Error> {
        match &self.connection_mode {
            ConnectionMode::File(path) => {
                let stream = Self::create_buffer_write_stream(path).await?;
                self.write_stream = Some(stream);
                self.connection_state = ConnectionState::Connected;
            }
            ConnectionMode::Tcp(host, port, use_ssl)
                if self.connection_state == ConnectionState::Disconnected =>
            {
                let stream =
                    Self::connect_to_remote(host, *use_ssl, self.ssl_config.clone()).await?;
                self.write_stream = Some(stream);
                self.connection_state = ConnectionState::Connected;
            }
            ConnectionMode::Bonjour(BonjourServiceType::Default(use_ssl))
                if self.connection_state == ConnectionState::Disconnected =>
            {
                self.browse_bonjour_services(*use_ssl)?;
            }
            _ => {
                // Nothing to do
            }
        };
        Ok(())
    }

    async fn try_reconnecting(&mut self) -> Result<(), Error> {
        if self.connection_state != ConnectionState::Disconnected {
            return Ok(());
        }

        #[cfg(test)]
        log::info!("try_reconnecting");
        self.setup_connection().await?;
        Ok(())
    }

    pub async fn create_buffer_write_stream(path: &Path) -> Result<WriteStreamWrapper, Error> {
        #[cfg(test)]
        log::info!("creating file buffer stream to {path:?}");
        let file_writer = BufWriter::new(File::create(path).await?);
        Ok(WriteStreamWrapper::File(file_writer))
    }

    pub async fn close_buffer_write_stream(&mut self) -> Result<(), Error> {
        #[cfg(test)]
        if self.write_stream.is_some() {
            log::info!("closing buffer stream");
        }

        if let Some(mut stream) = self.write_stream.take() {
            stream.flush().await?;
        };
        self.connection_state = ConnectionState::Disconnected;
        Ok(())
    }

    /// Write outstanding messages to the stream
    async fn write_messages_to_stream(&mut self) -> Result<(), Error> {
        #[cfg(test)]
        use crate::nslogger::log_message::SEQUENCE_NB_OFFSET;
        #[cfg(test)]
        log::info!(
            "process_log_queue: {} queued messages",
            self.log_messages.len()
        );

        while let Some((message, signal)) = self.log_messages.pop_front() {
            #[cfg(test)]
            log::info!(
                "processing message {}",
                &u32::from_be_bytes(
                    message.bytes()[SEQUENCE_NB_OFFSET..SEQUENCE_NB_OFFSET + 4]
                        .try_into()
                        .unwrap()
                )
            );

            let tcp_stream = self.write_stream.as_mut().unwrap();
            #[cfg(test)]
            log::info!(
                "writing to {:?} (len: {})",
                tcp_stream,
                message.bytes().len()
            );
            if let Err(_err) = tcp_stream.write_all(message.bytes()).await {
                self.log_messages.push_front((message, signal));
                #[cfg(test)]
                log::warn!("write to stream failed: {_err:?}");

                self.disconnect();
                self.try_reconnecting().await?;
                return Ok(());
            }
            if let Some(signal) = signal {
                tcp_stream.flush().await?;
                signal.signal();
            }
        }

        Ok(())
    }

    pub async fn process_log_queue(&mut self) -> Result<(), Error> {
        if self.log_messages.is_empty() {
            #[cfg(test)]
            log::info!("process_log_queue empty");
            return Ok(());
        }

        #[cfg(test)]
        log::info!("process_log_queue");

        if self.connection_state == ConnectionState::Disconnected {
            self.setup_connection().await?;
        }
        if self.connection_state == ConnectionState::Connected {
            self.push_client_info_to_front_of_queue();
        }
        if self.connection_state == ConnectionState::Ready {
            self.write_messages_to_stream().await?;
        }

        #[cfg(test)]
        log::info!("finished processing log queue",);
        Ok(())
    }
}

impl Drop for LogWorker {
    fn drop(&mut self) {
        #[cfg(test)]
        log::info!("calling drop for log worker");
        self.disconnect();
    }
}
