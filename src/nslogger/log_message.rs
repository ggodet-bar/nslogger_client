use std::{borrow::Cow, env, ffi::OsStr, fmt, path::Path, str::FromStr, thread, time};

pub(crate) const SEQUENCE_NB_OFFSET: usize = 14;

#[derive(Debug, PartialEq)]
pub enum Domain {
    App,
    View,
    Layout,
    Controller,
    Routing,
    Service,
    Network,
    Model,
    Cache,
    DB,
    IO,
    Custom(String),
}

impl fmt::Display for Domain {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Domain::Custom(ref custom_name) => write!(f, "{}", custom_name),
            _ => write!(f, "{:?}", self),
        }
    }
}

impl FromStr for Domain {
    type Err = ();

    fn from_str(s: &str) -> Result<Domain, ()> {
        let domain = match s {
            "App" => Domain::App,
            "View" => Domain::View,
            "Layout" => Domain::Layout,
            "Controller" => Domain::Controller,
            "Routing" => Domain::Routing,
            "Service" => Domain::Service,
            "Network" => Domain::Network,
            "Model" => Domain::Model,
            "Cache" => Domain::Cache,
            "DB" => Domain::DB,
            "IO" => Domain::IO,
            _ => Domain::Custom(s.to_string()),
        };
        Ok(domain)
    }
}

#[derive(Copy, Clone)]
#[repr(u8)]
#[allow(dead_code)]
pub enum MessagePartKey {
    MessageType = 0,
    /// "seconds" component of timestamp
    TimestampS = 1,
    /// Milliseconds component of timestamp (optional, mutually exclusive with TIMESTAMP_US)
    TimestampMs = 2,
    /// Microseconds component of timestamp (optional, mutually exclusive with TIMESTAMP_MS)
    TimestampUs = 3,
    ThreadId = 4,
    Tag = 5,
    Level = 6,
    Message = 7,
    /// Messages containing an image should also provide the image size
    ImageWidth = 8,
    ImageHeight = 9,
    /// Message sequence number.
    MessageSeq = 10,
    /*
     * Optional source file data.
     */
    FileName = 11,
    LineNumber = 12,
    FunctionName = 13,
    /*
     * Client info.
     */
    ClientName = 20,
    /// (unreachable in Rust)
    ClientVersion = 21,
    OsName = 22,
    OsVersion = 23,
    /// (Android-specific)
    ClientModel = 24,
    /// (Android-specific)
    UniqueId = 25,

    UserDefined = 100,
}

#[derive(Copy, Clone)]
#[repr(u8)]
pub(crate) enum MessagePartType {
    /// UTF-8 string
    String = 0,
    Binary = 1,
    Int16 = 2,
    Int32 = 3,
    Int64 = 4,
    /// PNG image
    Image = 5,
}

#[derive(Copy, Clone)]
#[repr(u32)]
#[allow(dead_code)]
pub(crate) enum LogMessageType {
    /// Standard log messagge
    Log = 0,
    /// Start of a block (i.e., a group) of log entries
    BlockStart,
    /// End of the block of entries
    BlockEnd,
    /// Client app info
    ClientInfo,
    /// Identifies a client disconnect
    Disconnect,
    /// Mark (i.e., a section marker) in the log sequence
    Mark,
}

/// Fully constructed log message. Log messages may be built from one of the 3 constructor calls
/// (`client_info`, `mark` or `log`) and are immutable, save for their sequence number, which is
/// sent upon dispatching the message to the `LogWorker`.
#[derive(Debug)]
pub struct LogMessage(Vec<u8>);

impl LogMessage {
    /// Returns a client info message.
    pub fn client_info() -> LogMessage {
        let process_name = env::current_exe()
            .ok()
            .as_ref()
            .map(Path::new)
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .map(String::from);
        LogMessageBuilder::new(LogMessageType::ClientInfo)
            .with_string_opt(MessagePartKey::OsName, sys_info::os_type().ok())
            .with_string_opt(MessagePartKey::OsVersion, sys_info::os_release().ok())
            .with_string_opt(MessagePartKey::ClientName, process_name)
            .freeze()
    }

    /// Returns a mark with the optional `message`.
    pub fn mark(message: Option<&str>) -> LogMessage {
        let mark_message = message.map(|msg| msg.to_string()).unwrap_or_else(|| {
            let time_now = chrono::Utc::now();
            time_now.format("%b %-d, %-I:%M:%S").to_string()
        });
        LogMessageBuilder::new(LogMessageType::Mark)
            .with_int16(MessagePartKey::Level, log::Level::Error as u16)
            .with_string(MessagePartKey::Message, &mark_message)
            .freeze()
    }

    /// Returns a log message builder.
    pub fn log<'a>() -> LogMessageBuilder<'a> {
        LogMessageBuilder::new(LogMessageType::Log)
    }

    /// Sets the message's unique sequence number, upon sending it to the log worker.
    pub fn set_sequence_number(&mut self, sequence_number: u32) {
        self.0[SEQUENCE_NB_OFFSET..(SEQUENCE_NB_OFFSET + 4)]
            .copy_from_slice(&sequence_number.to_be_bytes());
    }

    /// Returns a reference to the message's inner byte array.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

enum MessageData<'a> {
    Int64(u64),
    Int32(u32),
    Int16(u16),
    Bytes(MessagePartType, &'a [u8]),
    String(Cow<'a, str>),
}

impl<'a> MessageData<'a> {
    fn part_type(&self) -> MessagePartType {
        match self {
            Self::Int64(_) => MessagePartType::Int64,
            Self::Int32(_) => MessagePartType::Int32,
            Self::Int16(_) => MessagePartType::Int16,
            Self::String(_) => MessagePartType::String,
            Self::Bytes(part_type, _) => *part_type,
        }
    }
}

struct MessagePart<'a>(MessagePartKey, MessageData<'a>);

impl<'a> MessagePart<'a> {
    /// Returns the size of the serialized part, in bytes.
    fn size(&self) -> usize {
        match self {
            Self(_, MessageData::Int64(_)) => 10,
            Self(_, MessageData::Int32(_)) => 6,
            Self(_, MessageData::Int16(_)) => 4,
            Self(_, MessageData::String(string)) => 6 + string.len(),
            Self(_, MessageData::Bytes(_, bytes)) => 6 + bytes.len(),
        }
    }

    fn write(&mut self, buf: &mut [u8]) {
        buf[0] = self.0 as u8;
        buf[1] = self.1.part_type() as u8;
        match &self.1 {
            MessageData::Int64(v) => buf[2..10].copy_from_slice(&v.to_be_bytes()),
            MessageData::Int32(v) => buf[2..6].copy_from_slice(&v.to_be_bytes()),
            MessageData::Int16(v) => buf[2..4].copy_from_slice(&v.to_be_bytes()),
            MessageData::String(string) => {
                buf[2..6].copy_from_slice(&(string.len() as u32).to_be_bytes());
                buf[6..(6 + string.len())].copy_from_slice(string.as_bytes());
            }
            MessageData::Bytes(_, bytes) => {
                buf[2..6].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
                buf[6..(6 + bytes.len())].copy_from_slice(bytes);
            }
        }
    }
}

pub struct LogMessageBuilder<'a>(Vec<MessagePart<'a>>);

impl<'a> LogMessageBuilder<'a> {
    pub fn new(message_type: LogMessageType) -> LogMessageBuilder<'a> {
        LogMessageBuilder(Vec::with_capacity(8))
            /*
             * Message descriptor.
             */
            .with_int32(MessagePartKey::MessageType, message_type as u32)
            /*
             * Sequence number defaults to 0, as it will be set by the centralized worker.
             */
            .with_int32(MessagePartKey::MessageSeq, 0)
            .with_timestamp(None)
            .with_thread_id(thread::current())
    }

    fn with_int64(mut self, key: MessagePartKey, value: u64) -> Self {
        self.0.push(MessagePart(key, MessageData::Int64(value)));
        self
    }

    fn with_int32(mut self, key: MessagePartKey, value: u32) -> Self {
        self.0.push(MessagePart(key, MessageData::Int32(value)));
        self
    }

    fn with_int32_opt(self, key: MessagePartKey, value: Option<u32>) -> Self {
        let Some(value) = value else {
            return self;
        };
        self.with_int32(key, value)
    }

    pub(crate) fn with_int16(mut self, key: MessagePartKey, value: u16) -> Self {
        self.0.push(MessagePart(key, MessageData::Int16(value)));
        self
    }

    fn with_bytes(
        mut self,
        key: MessagePartKey,
        data_type: MessagePartType,
        bytes: &'a [u8],
    ) -> Self {
        self.0
            .push(MessagePart(key, MessageData::Bytes(data_type, bytes)));
        self
    }

    pub fn with_string(mut self, key: MessagePartKey, stringlike: impl Into<Cow<'a, str>>) -> Self {
        self.0
            .push(MessagePart(key, MessageData::String(stringlike.into())));
        self
    }

    pub fn with_string_opt(
        self,
        key: MessagePartKey,
        stringlike: Option<impl Into<Cow<'a, str>>>,
    ) -> Self {
        let Some(string) = stringlike else {
            return self;
        };
        self.with_string(key, string)
    }

    /// Current size of the serialized message, in bytes.
    fn size(&self) -> usize {
        self.0.iter().map(MessagePart::size).sum::<usize>() + 6
    }

    pub fn with_header(
        self,
        filename: Option<&Path>,
        line_number: Option<u32>,
        method: Option<&'a str>,
        domain: Option<Domain>,
        level: log::Level,
    ) -> Self {
        self.with_int16(MessagePartKey::Level, level as u16)
            .with_string_opt(
                MessagePartKey::FileName,
                filename.map(|p| p.to_string_lossy().into_owned()),
            )
            .with_int32_opt(MessagePartKey::LineNumber, filename.and(line_number))
            .with_string_opt(MessagePartKey::FunctionName, method)
            .with_string_opt(MessagePartKey::Tag, domain.map(|d| d.to_string()))
    }

    pub fn with_binary_data(self, key: MessagePartKey, bytes: &'a [u8]) -> Self {
        self.with_bytes(key, MessagePartType::Binary, bytes)
    }

    pub fn with_image_data(self, key: MessagePartKey, bytes: &'a [u8]) -> Self {
        self.with_bytes(key, MessagePartType::Image, bytes)
    }

    /// Logs the passed `ts`, otherwise logs the 'now' timestamp.
    fn with_timestamp(self, ts: Option<u64>) -> Self {
        let value = ts.unwrap_or_else(|| {
            time::SystemTime::now()
                .duration_since(time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        });
        self.with_int64(MessagePartKey::TimestampS, value / 1000)
            .with_int16(MessagePartKey::TimestampMs, (value % 1000) as u16)
    }

    fn with_thread_id(self, thread: thread::Thread) -> Self {
        self.with_string(
            MessagePartKey::ThreadId,
            Cow::Owned(format!("{:?}", thread.id())),
        )
    }

    pub fn freeze(mut self) -> LogMessage {
        let mut buffer = vec![0_u8; self.size()];
        /*
         * NOTE It is a protocol requirement that the size value be decremented by 4.
         */
        buffer[..4].copy_from_slice(&((self.size() - 4) as u32).to_be_bytes());
        /*
         * Copy the part count into the buffer.
         */
        buffer[4..6].copy_from_slice(&(self.0.len() as u16).to_be_bytes());
        /*
         * Serialize each part into the buffer.
         */
        let mut idx = 6;
        for part in &mut self.0 {
            part.write(&mut buffer[idx..(idx + part.size())]);
            idx += part.size();
        }
        /*
         * Wrap and return as a log message.
         */
        LogMessage(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl<'a> MessageData<'a> {
        fn as_int64(&self) -> Option<u64> {
            let Self::Int64(v) = self else {
                return None;
            };
            return Some(*v);
        }

        fn as_int16(&self) -> Option<u16> {
            let Self::Int16(v) = self else {
                return None;
            };
            return Some(*v);
        }
    }

    #[test]
    fn builds_smallest_message() {
        let thread_id_str = format!("{:?}", std::thread::current().id());
        let id_string_len = thread_id_str.len();
        let builder = LogMessageBuilder::new(LogMessageType::Log);
        assert_eq!(5, builder.0.len());
        assert_eq!(38 + id_string_len, builder.size());
        let ts_s = builder.0[2].1.as_int64().unwrap();
        let ts_ms = builder.0[3].1.as_int16().unwrap();
        let msg = builder.freeze();

        let mut expected = Vec::with_capacity(38 + id_string_len);
        /*
         * Push the msg header (6B | 0B..6B).
         */
        expected.extend_from_slice(&(34 + id_string_len as u32).to_be_bytes());
        expected.extend_from_slice(&5_u16.to_be_bytes());
        /*
         * Push the message type (6B | 6B..12B).
         */
        expected.push(MessagePartKey::MessageType as u8);
        expected.push(MessagePartType::Int32 as u8);
        expected.extend(&(LogMessageType::Log as u32).to_be_bytes());
        /*
         * Push a null sequence number (6B | 12B..18B).
         */
        expected.push(MessagePartKey::MessageSeq as u8);
        expected.push(MessagePartType::Int32 as u8);
        expected.extend(&[0_u8; 4]);
        /*
         * Push the timestamp seconds (10B | 18B..28B).
         */
        expected.push(MessagePartKey::TimestampS as u8);
        expected.push(MessagePartType::Int64 as u8);
        expected.extend(&ts_s.to_be_bytes());
        /*
         * Push the timestamp milliseconds (4B | 28B..32B).
         */
        expected.push(MessagePartKey::TimestampMs as u8);
        expected.push(MessagePartType::Int16 as u8);
        expected.extend(&ts_ms.to_be_bytes());
        /*
         * Push the thread id. (6B + len(str) | 32B..(38 + len(str)B))
         */
        expected.push(MessagePartKey::ThreadId as u8);
        expected.push(MessagePartType::String as u8);
        expected.extend(&(id_string_len as u32).to_be_bytes());
        expected.extend(thread_id_str.as_bytes());
        /*
         * Compare.
         */
        assert_eq!(msg.bytes(), &expected);
    }

    #[test]
    fn parses_domain_from_string() {
        use std::str::FromStr;
        assert_eq!(Domain::App, Domain::from_str("App").unwrap());
        assert_eq!(Domain::DB, Domain::from_str("DB").unwrap());
        assert_eq!(
            Domain::Custom("CustomTag".to_string()),
            Domain::from_str("CustomTag").unwrap()
        );
    }
}
