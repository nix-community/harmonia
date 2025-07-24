pub mod messages;
pub mod opcodes;
pub mod version;

pub use messages::{
    LoggerField, Msg, StderrError, StderrStartActivity, StorePath, Trace, ValidPathInfo,
};
pub use opcodes::OpCode;
pub use version::{ProtocolVersion, CURRENT_PROTOCOL_VERSION, MIN_PROTOCOL_VERSION};

pub const WORKER_MAGIC_1: u64 = 0x6e697863;
pub const WORKER_MAGIC_2: u64 = 0x6478696f;

pub const MAX_STRING_SIZE: u64 = 0x1000000; // 16M
pub const MAX_STRING_LIST_SIZE: u64 = 0x10000; // 64K
