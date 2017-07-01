use std::sync::mpsc ;
use std::thread ;

use std::time ;
use std::path::Path ;
use std::fmt ;
use std::str::FromStr ;

use byteorder ;
use byteorder::{ByteOrder, NetworkEndian, WriteBytesExt} ;

use log ;

#[derive(Debug,PartialEq)]
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
  Custom(String)
}

impl fmt::Display for Domain {
    fn fmt(&self, f:&mut fmt::Formatter) -> fmt::Result {
        match *self {
            Domain::Custom(ref custom_name) => write!(f, "{}", custom_name),
            _ => write!(f, "{:?}", self)
        }
    }
}

impl FromStr for Domain {
    type Err = () ;

    fn from_str(s: &str) -> Result<Domain, ()> {
        match s {
            "App" => return Ok(Domain::App),
            "View" => return Ok(Domain::View),
            "Layout" => return Ok(Domain::Layout),
            "Controller" => return Ok(Domain::Controller),
            "Routing" => return Ok(Domain::Routing),
            "Service" => return Ok(Domain::Service),
            "Network" => return Ok(Domain::Network),
            "Model" => return Ok(Domain::Model),
            "Cache" => return Ok(Domain::Cache),
            "DB" => return Ok(Domain::DB),
            "IO" => return Ok(Domain::IO),
            _ => return Ok(Domain::Custom(s.to_string()))
        }
    }
}

#[derive(Copy,Clone)]
pub enum Level {
    Error = 0,
    Warning,
    Important,
    Info,
    Debug,
    Verbose,
    Noise
}

impl Level {
    pub fn from_log_level(level:log::LogLevel) -> Level {
        match level {
            log::LogLevel::Error => Level::Error,
            log::LogLevel::Warn  => Level::Warning,
            log::LogLevel::Info  => Level::Info,
            log::LogLevel::Debug => Level::Debug,
            log::LogLevel::Trace => Level::Verbose,
        }
    }
}

#[derive(Copy,Clone)]
pub enum MessagePartKey {
    MESSAGE_TYPE  = 0,
    TIMESTAMP_S   = 1,    // "seconds" component of timestamp
    TIMESTAMP_MS  = 2,    // milliseconds component of timestamp (optional, mutually exclusive with TIMESTAMP_US)
    TIMESTAMP_US  = 3,    // microseconds component of timestamp (optional, mutually exclusive with TIMESTAMP_MS)
    THREAD_ID     = 4,
    TAG           = 5,
    LEVEL         = 6,
    MESSAGE       = 7,
    IMAGE_WIDTH   = 8,    // messages containing an image should also contain a part with the image size
    IMAGE_HEIGHT  = 9,    // (this is mainly for the desktop viewer to compute the cell size without having to immediately decode the image)
    MESSAGE_SEQ   = 10,   // the sequential number of this message which indicates the order in which messages are generated
    FILENAME      = 11,   // when logging, message can contain a file name
    LINENUMBER    = 12,   // as well as a line number
    FUNCTIONNAME  = 13,   // and a function or method name
}

#[derive(Copy,Clone)]
enum MessagePartType {
    STRING = 0,     // Strings are stored as UTF-8 data
    BINARY = 1,     // A block of binary data
    INT16 = 2,
    INT32 = 3,
    INT64 = 4,
    IMAGE = 5,      // An image, stored in PNG format
}

#[derive(Copy,Clone)]
pub enum LogMessageType {
    LOG = 0,               // A standard log message
    BLOCK_START,       // The start of a "block" (a group of log entries)
    BLOCK_END,         // The end of the last started "block"
    CLIENT_INFO,       // Information about the client app
    DISCONNECT,        // Pseudo-message on the desktop side to identify client disconnects
    MARK               // Pseudo-message that defines a "mark" that users can place in the log flow
}




#[derive(Debug)]
pub struct LogMessage {
    pub sequence_number:u32,
    data:Vec<u8>,
    data_used:u32,
    part_count:u16,
    pub flush_rx:Option<mpsc::Receiver<bool>>,
    pub flush_tx:mpsc::Sender<bool>
}

impl LogMessage {
    pub fn new(message_type:LogMessageType, sequence_number:u32) -> LogMessage {
        let (flush_tx, flush_rx) = mpsc::channel() ;
        let mut new_message = LogMessage { sequence_number:sequence_number,
                                           data:Vec::with_capacity(256),
                                           data_used:6,
                                           part_count:0,
                                           flush_rx: Some(flush_rx),
                                           flush_tx: flush_tx } ;

        new_message.add_int32(MessagePartKey::MESSAGE_TYPE, message_type as u32) ;
        new_message.add_int32(MessagePartKey::MESSAGE_SEQ, sequence_number) ;
        new_message.add_timestamp(0) ;
        new_message.add_thread_id(thread::current()) ;

        new_message
    }

    pub fn with_header(message_type:LogMessageType, sequence_number:u32, filename:Option<&Path>,
                       line_number:Option<usize>, method:Option<&str>, domain:Option<Domain>, level:Level) -> LogMessage {
        let mut new_message = LogMessage::new(message_type, sequence_number) ;

        new_message.add_int16(MessagePartKey::LEVEL, level as u16) ;

        if let Some(path) = filename {
            new_message.add_string(MessagePartKey::FILENAME, path.to_str().expect("Invalid path encoding")) ;

            if let Some(nb) = line_number {
                new_message.add_int32(MessagePartKey::LINENUMBER, nb as u32) ;
            }
        }

        if let Some(method_name) = method {
            new_message.add_string(MessagePartKey::FUNCTIONNAME, method_name) ;
        }

        if let Some(domain_tag) = domain {
            let tag_string = domain_tag.to_string() ;
            if !tag_string.is_empty() {
                new_message.add_string(MessagePartKey::TAG, &tag_string) ;
            }
        } ;
        new_message
    }

    pub fn add_int64(&mut self, key:MessagePartKey, value:u64) {
        self.data_used += 10 ;
        self.data.write_u8(key as u8).unwrap() ;
        self.data.write_u8(MessagePartType::INT64 as u8).unwrap() ;
        self.data.write_u64::<NetworkEndian>(value as u64).unwrap() ;
        self.part_count += 1 ;
    }

    pub fn add_int32(&mut self, key:MessagePartKey, value:u32) {
        self.data_used += 6 ;
        self.data.write_u8(key as u8).unwrap() ;
        self.data.write_u8(MessagePartType::INT32 as u8).unwrap() ;
        self.data.write_u32::<NetworkEndian>(value as u32).unwrap() ;
        self.part_count += 1 ;
    }

    pub fn add_int16(&mut self, key:MessagePartKey, value:u16) {
        self.data_used += 4 ;
        self.data.write_u8(key as u8).unwrap() ;
        self.data.write_u8(MessagePartType::INT16 as u8).unwrap() ;
        self.data.write_u16::<NetworkEndian>(value as u16).unwrap() ;
        self.part_count += 1 ;
    }

    pub fn add_binary_data(&mut self, key:MessagePartKey, bytes:&[u8]) {
        self.add_bytes(key, MessagePartType::BINARY, bytes) ;
    }

    pub fn add_image_data(&mut self, key:MessagePartKey, bytes:&[u8]) {
        self.add_bytes(key, MessagePartType::IMAGE, bytes) ;
    }

    fn add_bytes(&mut self, key:MessagePartKey, data_type:MessagePartType, bytes:&[u8]) {
        let length = bytes.len() ;
        self.data_used += (6 + length) as u32 ;
        self.data.write_u8(key as u8).unwrap() ;
        self.data.write_u8(data_type as u8).unwrap() ;
        self.data.write_u32::<NetworkEndian>(length as u32).unwrap() ;
        self.data.extend_from_slice(bytes) ;
        self.part_count += 1 ;
    }

    pub fn add_string(&mut self, key:MessagePartKey, string:&str) {
        self.add_bytes(key, MessagePartType::STRING, string.as_bytes()) ;
    }

    fn add_timestamp(&mut self, value:u64) {
        let actual_value = if value == 0 {
            let time = time::SystemTime::now().duration_since(time::UNIX_EPOCH).expect("Time went backward") ;
            (time.as_secs() * 1000 + time.subsec_nanos() as u64 / 1_000_000) as u64
        } else {
            value
        } ;

        self.add_int64(MessagePartKey::TIMESTAMP_S, actual_value / 1000) ;
        self.add_int16(MessagePartKey::TIMESTAMP_MS, (actual_value % 1000) as u16) ;
    }

    fn add_thread_id(&mut self, thread:thread::Thread) {
        let thread_name = match thread.name() {
            Some(name) => name.to_string(),
            None => format!("{:?}", thread.id())
        } ;

        self.add_string(MessagePartKey::THREAD_ID, &thread_name) ;
    }

    pub fn get_bytes(&self) -> Vec<u8> {
        let mut header = Vec::with_capacity(6 + self.data.len()) ;
        let size = self.data_used - 4 ;
        header.write_u32::<NetworkEndian>(size) ;
        header.write_u16::<NetworkEndian>(self.part_count) ;

        if self.data_used == self.data.len() as u32 {
            return self.data.clone() ;
        }

        header.extend_from_slice(&self.data) ;

        return header ;
    }
}
