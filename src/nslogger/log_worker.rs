use std::{
    collections::VecDeque,
    fs::File,
    io,
    io::{BufWriter, Write},
    net::TcpStream,
    path::{Path, PathBuf},
};

use openssl::{
    self,
    ssl::{SslConnector, SslMethod, SslStream},
};
use tokio::sync::mpsc;

use crate::nslogger::{
    log_message::{LogMessage, SEQUENCE_NB_OFFSET},
    network_manager,
    network_manager::BonjourServiceType,
    Error, Signal, DEBUG_LOGGER,
};

#[derive(Debug)]
pub enum Message {
    ConnectToBonjourService(String, u16, bool),
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
        /*
         * NOTE the worker won't process the client info message, hence the very first message
         * is skipped.
         */
        Self {
            sequence_generator: 1,
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
        if DEBUG_LOGGER {
            log::info!("logging thread starting up");
        }
        /*
         * Initial setup according to current parameters.
         */
        self.setup_connection()?;
        if DEBUG_LOGGER {
            log::info!("starting log event loop");
        }

        self.run_loop().await?;

        if DEBUG_LOGGER {
            log::info!("stopped log event loop");
        }
        Ok(())
    }

    fn handle_message(&mut self, message: Message) -> Result<(), Error> {
        if DEBUG_LOGGER {
            log::info!("received message");
        }

        match message {
            Message::AddLog(mut message, signal) => {
                /*
                 * Sequence number is set on receiving the message in the handler to
                 * guarantee a strictly monotonic sequence.
                 */
                message.sequence_number = self.sequence_generator;
                message.data[SEQUENCE_NB_OFFSET..(SEQUENCE_NB_OFFSET + 4)]
                    .copy_from_slice(&self.sequence_generator.to_be_bytes());
                self.sequence_generator += 1;
                if DEBUG_LOGGER {
                    log::info!("adding log {} to the queue", message.sequence_number);
                }

                self.log_messages.push_back((message, signal));
                self.process_log_queue()?;
            }
            Message::SwitchConnection(new_mode) => {
                self.change_options(new_mode)?;
            }
            Message::ConnectToBonjourService(host, port, use_ssl) => {
                let stream = self.connect_to_remote(&host, port, use_ssl)?;
                self.write_stream = Some(stream);
                self.process_log_queue()?;
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
            self.handle_message(message)?;
        }

        self.close_buffer_write_stream()?;

        Ok(())
    }

    fn push_client_info_to_front_of_queue(&mut self) {
        if DEBUG_LOGGER {
            log::info!("pushing client info to front of queue");
        }

        self.log_messages
            .push_front((LogMessage::client_info(), None));
        self.connection_state = ConnectionState::Ready;
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

        self.connection_state = ConnectionState::Connecting;

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
            let mut ssl_connector_builder = SslConnector::builder(SslMethod::tls()).unwrap();

            ssl_connector_builder.set_verify(openssl::ssl::SslVerifyMode::NONE);
            ssl_connector_builder
                .set_verify_callback(openssl::ssl::SslVerifyMode::NONE, |_, _| true);

            let connector = ssl_connector_builder.build();
            let stream = connector.connect("localhost", stream).unwrap();
            if DEBUG_LOGGER {
                log::info!("opened SSL stream");
            }
            WriteStreamWrapper::Ssl(stream)
        } else {
            WriteStreamWrapper::Tcp(stream)
        };

        self.connection_state = ConnectionState::Connected;

        Ok(stream)
    }

    pub fn disconnect(&mut self) {
        if DEBUG_LOGGER {
            log::info!("disconnect_from_remote()");
        }

        self.connection_state = ConnectionState::Disconnected;
        self.write_stream = None;
        self.sequence_generator = 1;
    }

    pub fn setup_connection(&mut self) -> Result<(), Error> {
        match self.connection_mode.clone() {
            ConnectionMode::File(path) => {
                let stream = self.create_buffer_write_stream(&path)?;
                self.write_stream = Some(stream);
            }
            ConnectionMode::Tcp(host, port, use_ssl)
                if self.connection_state == ConnectionState::Disconnected =>
            {
                let stream = self.connect_to_remote(&host, port, use_ssl)?;
                self.write_stream = Some(stream);
            }
            ConnectionMode::Bonjour(BonjourServiceType::Default(use_ssl))
                if self.connection_state == ConnectionState::Disconnected =>
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
        if self.connection_state != ConnectionState::Disconnected {
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
        self.connection_state = ConnectionState::Connected;
        Ok(WriteStreamWrapper::File(file_writer))
    }

    pub fn close_buffer_write_stream(&mut self) -> Result<(), Error> {
        if DEBUG_LOGGER && self.write_stream.is_some() {
            log::info!("closing buffer stream");
        }

        if let Some(mut stream) = self.write_stream.take() {
            stream.flush()?;
        };
        self.connection_state = ConnectionState::Disconnected;
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

        if self.connection_state == ConnectionState::Disconnected {
            self.setup_connection()?;
        }
        if self.connection_state == ConnectionState::Connected {
            self.push_client_info_to_front_of_queue();
        }
        if self.connection_state == ConnectionState::Ready {
            self.write_messages_to_stream()?;
        }

        if DEBUG_LOGGER {
            log::info!("finished processing log queue",);
        }
        Ok(())
    }
}

impl Drop for LogWorker {
    fn drop(&mut self) {
        if DEBUG_LOGGER {
            log::info!("calling drop for log worker");
        }
        if let Some(mut stream) = self.write_stream.take() {
            stream.flush().unwrap();
        }
        self.disconnect();
    }
}
