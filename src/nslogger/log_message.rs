use std::{env, ffi::OsStr, fmt, path::Path, str::FromStr, thread, time};

use byteorder::{BigEndian, WriteBytesExt};
use log;
use sys_info;

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
pub enum Level {
    Error = 0,
    Warning,
    Important,
    Info,
    Debug,
    Verbose,
    Noise,
}

impl Level {
    pub fn from_log_level(level: log::LogLevel) -> Level {
        match level {
            log::LogLevel::Error => Level::Error,
            log::LogLevel::Warn => Level::Warning,
            log::LogLevel::Info => Level::Info,
            log::LogLevel::Debug => Level::Debug,
            log::LogLevel::Trace => Level::Verbose,
        }
    }
}

#[derive(Copy, Clone)]
#[repr(u8)]
pub enum MessagePartKey {
    MessageType = 0,
    TimestampS = 1, // "seconds" component of timestamp
    TimestampMs = 2, /* milliseconds component of timestamp (optional, mutually exclusive with
                     * TIMESTAMP_US) */
    TimestampUs = 3, /* microseconds component of timestamp (optional, mutually exclusive with
                      * TIMESTAMP_MS) */
    ThreadId = 4,
    Tag = 5,
    Level = 6,
    Message = 7,
    ImageWidth = 8, // messages containing an image should also contain a part with the image size
    ImageHeight = 9, /* (this is mainly for the desktop viewer to compute the cell size without
                     * having to immediately decode the image) */
    MessageSeq = 10, /* the sequential number of this message which indicates the order in
                      * which messages are generated */
    FileName = 11,     // when logging, message can contain a file name
    LineNumber = 12,   // as well as a line number
    FunctionName = 13, // and a function or method name

    // Client info
    ClientName = 20,
    ClientVersion = 21, // unreachable from Rust
    OsName = 22,
    OsVersion = 23,
    ClientModel = 24, // Android-specific
    UniqueId = 25,    // Android-specific

    UserDefined = 100,
}

#[derive(Copy, Clone)]
#[repr(u8)]
pub(crate) enum MessagePartType {
    String = 0, // Strings are stored as UTF-8 data
    Binary = 1, // A block of binary data
    Int16 = 2,
    Int32 = 3,
    Int64 = 4,
    Image = 5, // An image, stored in PNG format
}

#[derive(Copy, Clone)]
#[repr(u32)]
pub(crate) enum LogMessageType {
    Log = 0,    // A standard log message
    BlockStart, // The start of a "block" (a group of log entries)
    BlockEnd,   // The end of the last started "block"
    ClientInfo, // Information about the client app
    Disconnect, // Pseudo-message on the desktop side to identify client disconnects
    Mark,       // Pseudo-message that defines a "mark" that users can place in the log flow
}

#[derive(Debug)]
pub struct LogMessage {
    pub sequence_number: u32,
    pub data: Vec<u8>,
    part_count: u16,
}

impl Default for LogMessage {
    fn default() -> Self {
        Self {
            sequence_number: 0,
            part_count: 0,
            data: Vec::with_capacity(512),
        }
    }
}

impl LogMessage {
    pub fn client_info() -> LogMessage {
        let mut message = LogMessage::new(LogMessageType::ClientInfo);

        if let Ok(os_type) = sys_info::os_type() {
            message.add_string(MessagePartKey::OsName, &os_type);
        };
        if let Ok(os_release) = sys_info::os_release() {
            message.add_string(MessagePartKey::OsVersion, &os_release);
        }
        let process_name = env::current_exe()
            .ok()
            .as_ref()
            .map(Path::new)
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            .map(String::from);

        if let Some(name) = process_name {
            message.add_string(MessagePartKey::ClientName, &name);
        }

        message
    }

    pub fn new(message_type: LogMessageType) -> LogMessage {
        let mut new_message = LogMessage::default();
        /*
         * Reserve 6 bytes for the message header.
         */
        new_message.data.extend_from_slice(&[0_u8; 6]);
        /*
         * Message descriptor.
         */
        new_message.add_int32(MessagePartKey::MessageType, message_type as u32);
        new_message.add_int32(MessagePartKey::MessageSeq, 0);
        new_message.add_timestamp(None);
        new_message.add_thread_id(thread::current());
        new_message
    }

    pub fn with_header(
        message_type: LogMessageType,
        filename: Option<&Path>,
        line_number: Option<usize>,
        method: Option<&str>,
        domain: Option<Domain>,
        level: Level,
    ) -> LogMessage {
        let mut new_message = LogMessage::new(message_type);

        new_message.add_int16(MessagePartKey::Level, level as u16);

        if let Some(path) = filename {
            new_message.add_string(
                MessagePartKey::FileName,
                path.to_str().expect("Invalid path encoding"),
            );

            if let Some(nb) = line_number {
                new_message.add_int32(MessagePartKey::LineNumber, nb as u32);
            }
        }

        if let Some(method_name) = method {
            new_message.add_string(MessagePartKey::FunctionName, method_name);
        }

        if let Some(domain_tag) = domain {
            let tag_string = domain_tag.to_string();
            if !tag_string.is_empty() {
                new_message.add_string(MessagePartKey::Tag, &tag_string);
            }
        };
        new_message
    }

    pub fn add_int64(&mut self, key: MessagePartKey, value: u64) {
        self.data.write_u8(key as u8).unwrap();
        self.data.write_u8(MessagePartType::Int64 as u8).unwrap();
        self.data.write_u64::<BigEndian>(value).unwrap();
        self.part_count += 1;
    }

    pub fn add_int32(&mut self, key: MessagePartKey, value: u32) {
        self.data.write_u8(key as u8).unwrap();
        self.data.write_u8(MessagePartType::Int32 as u8).unwrap();
        self.data.write_u32::<BigEndian>(value).unwrap();
        self.part_count += 1;
    }

    pub fn add_int16(&mut self, key: MessagePartKey, value: u16) {
        self.data.write_u8(key as u8).unwrap();
        self.data.write_u8(MessagePartType::Int16 as u8).unwrap();
        self.data.write_u16::<BigEndian>(value).unwrap();
        self.part_count += 1;
    }

    pub fn add_binary_data(&mut self, key: MessagePartKey, bytes: &[u8]) {
        self.add_bytes(key, MessagePartType::Binary, bytes);
    }

    pub fn add_image_data(&mut self, key: MessagePartKey, bytes: &[u8]) {
        self.add_bytes(key, MessagePartType::Image, bytes);
    }

    fn add_bytes(&mut self, key: MessagePartKey, data_type: MessagePartType, bytes: &[u8]) {
        let length = bytes.len();
        self.data.write_u8(key as u8).unwrap();
        self.data.write_u8(data_type as u8).unwrap();
        self.data.write_u32::<BigEndian>(length as u32).unwrap();
        self.data.extend_from_slice(bytes);
        self.part_count += 1;
    }

    pub fn add_string(&mut self, key: MessagePartKey, string: &str) {
        self.add_bytes(key, MessagePartType::String, string.as_bytes());
    }

    fn add_timestamp(&mut self, value: Option<u64>) {
        let value = value.unwrap_or_else(|| {
            time::SystemTime::now()
                .duration_since(time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        });
        self.add_int64(MessagePartKey::TimestampS, value / 1000);
        self.add_int16(MessagePartKey::TimestampMs, (value % 1000) as u16);
    }

    fn add_thread_id(&mut self, thread: thread::Thread) {
        let thread_name = thread
            .name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| format!("{:?}", thread.id()));
        self.add_string(MessagePartKey::ThreadId, &thread_name);
    }

    pub fn freeze(&mut self) {
        let size = self.data.len() as u32 - 4;
        let data_slice = self.data.as_mut_slice();
        data_slice[..4].copy_from_slice(&size.to_be_bytes());
        data_slice[4..6].copy_from_slice(&self.part_count.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smallest_message() {
        let thread_name_len = std::thread::current().name().unwrap().len();
        let mut msg = LogMessage::new(LogMessageType::Log);
        assert_eq!(5, msg.part_count);
        assert_eq!(38 + thread_name_len, msg.data.len());
        msg.freeze();
        let bytes = msg.data;
        assert_eq!(
            34 + thread_name_len as u32,
            u32::from_be_bytes(bytes[0..4].try_into().unwrap())
        );
        assert_eq!(5, u16::from_be_bytes(bytes[4..6].try_into().unwrap()));
        assert_eq!(38 + thread_name_len, bytes.len());
    }
}
