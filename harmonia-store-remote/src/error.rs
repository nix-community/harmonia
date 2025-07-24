use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProtocolError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid magic number: expected {expected:#x}, got {actual:#x}")]
    InvalidMagic { expected: u64, actual: u64 },

    #[error("Protocol version mismatch: server version {server} is incompatible with client range {min}-{max}")]
    IncompatibleVersion {
        server: crate::protocol::ProtocolVersion,
        min: crate::protocol::ProtocolVersion,
        max: crate::protocol::ProtocolVersion,
    },

    #[error("String too long: {length} exceeds maximum {max}")]
    StringTooLong { length: u64, max: u64 },

    #[error("Invalid operation code: {0}")]
    InvalidOpCode(u64),

    #[error("Daemon error: {message}")]
    DaemonError { message: String },

    #[error("String list too long: {length} exceeds maximum {max}")]
    StringListTooLong { length: u64, max: u64 },

    #[error("Invalid message code: {0:#x}")]
    InvalidMsgCode(u64),
}
