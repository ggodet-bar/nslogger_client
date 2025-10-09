use std::{env, ffi::OsStr, fmt, path::Path, str::FromStr, thread, time};

use byteorder::{BigEndian, WriteBytesExt};

pub const SEQUENCE_NB_OFFSET: usize = 14;

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

#[derive(Debug)]
pub struct LogMessage {
    pub data: Vec<u8>,
    /// Number of parts contained in the log message
    part_count: u16,
}

impl Default for LogMessage {
    fn default() -> Self {
        Self {
            part_count: 0,
            data: Vec::with_capacity(512),
        }
    }
}

impl LogMessage {
    pub fn client_info() -> LogMessage {
        let process_name = env::current_exe()
            .ok()
            .as_ref()
            .map(Path::new)
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .map(String::from);
        LogMessage::new(LogMessageType::ClientInfo)
            .with_string_opt(MessagePartKey::OsName, sys_info::os_type().ok())
            .with_string_opt(MessagePartKey::OsVersion, sys_info::os_release().ok())
            .with_string_opt(MessagePartKey::ClientName, process_name)
    }

    pub fn new(message_type: LogMessageType) -> LogMessage {
        LogMessage::default()
            /*
             * Reserve 6 bytes for the message header.
             */
            .with_reserved_bytes(6)
            /*
             * Message descriptor.
             */
            .with_int32(MessagePartKey::MessageType, message_type as u32)
            .with_int32(MessagePartKey::MessageSeq, 0)
            .with_timestamp(None)
            .with_thread_id(thread::current())
    }

    pub fn with_header(
        message_type: LogMessageType,
        filename: Option<&Path>,
        line_number: Option<u32>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: log::Level,
    ) -> LogMessage {
        LogMessage::new(message_type)
            .with_int16(MessagePartKey::Level, level as u16)
            .with_string_opt(
                MessagePartKey::FileName,
                filename.map(|p| p.to_string_lossy().into_owned()),
            )
            .with_int32_opt(MessagePartKey::LineNumber, filename.and(line_number))
            .with_string_opt(MessagePartKey::FunctionName, method)
            .with_string_opt(MessagePartKey::Tag, domain.map(|d| d.to_string()))
    }

    fn with_reserved_bytes(mut self, len: usize) -> Self {
        self.data.extend_from_slice(&vec![0_u8; len]);
        self
    }

    fn with_int64(mut self, key: MessagePartKey, value: u64) -> Self {
        self.data.write_u8(key as u8).unwrap();
        self.data.write_u8(MessagePartType::Int64 as u8).unwrap();
        self.data.write_u64::<BigEndian>(value).unwrap();
        self.part_count += 1;
        self
    }

    fn with_int32(mut self, key: MessagePartKey, value: u32) -> Self {
        self.data.write_u8(key as u8).unwrap();
        self.data.write_u8(MessagePartType::Int32 as u8).unwrap();
        self.data.write_u32::<BigEndian>(value).unwrap();
        self.part_count += 1;
        self
    }

    fn with_int32_opt(self, key: MessagePartKey, value: Option<u32>) -> Self {
        let Some(value) = value else {
            return self;
        };
        self.with_int32(key, value)
    }

    pub(crate) fn with_int16(mut self, key: MessagePartKey, value: u16) -> Self {
        self.data.write_u8(key as u8).unwrap();
        self.data.write_u8(MessagePartType::Int16 as u8).unwrap();
        self.data.write_u16::<BigEndian>(value).unwrap();
        self.part_count += 1;
        self
    }

    fn with_bytes(mut self, key: MessagePartKey, data_type: MessagePartType, bytes: &[u8]) -> Self {
        let length = bytes.len();
        self.data.write_u8(key as u8).unwrap();
        self.data.write_u8(data_type as u8).unwrap();
        self.data.write_u32::<BigEndian>(length as u32).unwrap();
        self.data.extend_from_slice(bytes);
        self.part_count += 1;
        self
    }

    pub fn with_binary_data(self, key: MessagePartKey, bytes: &[u8]) -> Self {
        self.with_bytes(key, MessagePartType::Binary, bytes)
    }

    pub fn with_image_data(self, key: MessagePartKey, bytes: &[u8]) -> Self {
        self.with_bytes(key, MessagePartType::Image, bytes)
    }

    pub fn with_string<A: AsRef<str>>(self, key: MessagePartKey, stringlike: A) -> Self {
        self.with_bytes(key, MessagePartType::String, stringlike.as_ref().as_bytes())
    }

    pub fn with_string_opt<A: AsRef<str>>(
        self,
        key: MessagePartKey,
        stringlike: Option<A>,
    ) -> Self {
        let Some(string) = stringlike else {
            return self;
        };
        self.with_string(key, string)
    }

    fn with_timestamp(self, value: Option<u64>) -> Self {
        let value = value.unwrap_or_else(|| {
            time::SystemTime::now()
                .duration_since(time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        });
        self.with_int64(MessagePartKey::TimestampS, value / 1000)
            .with_int16(MessagePartKey::TimestampMs, (value % 1000) as u16)
    }

    fn with_thread_id(self, thread: thread::Thread) -> Self {
        self.with_string(MessagePartKey::ThreadId, &format!("{:?}", thread.id()))
    }

    pub fn freeze(mut self) -> Self {
        let size = self.data.len() as u32 - 4;
        let data_slice = self.data.as_mut_slice();
        data_slice[..4].copy_from_slice(&size.to_be_bytes());
        data_slice[4..6].copy_from_slice(&self.part_count.to_be_bytes());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smallest_message() {
        let id_string_len = format!("{:?}", std::thread::current().id()).len();
        let msg = LogMessage::new(LogMessageType::Log);
        assert_eq!(5, msg.part_count);
        assert_eq!(38 + id_string_len, msg.data.len());
        assert_eq!(&[0_u8; 6], &msg.data[..6]);
        let bytes = msg.freeze().data;
        assert_eq!(
            34 + id_string_len as u32,
            u32::from_be_bytes(bytes[0..4].try_into().unwrap())
        );
        assert_eq!(5, u16::from_be_bytes(bytes[4..6].try_into().unwrap()));
        assert_eq!(38 + id_string_len, bytes.len());
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
