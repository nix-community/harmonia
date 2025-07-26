use thiserror::Error;

#[derive(Error, Debug)]
pub enum ProtocolError {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Invalid magic number: expected {expected:#x}, got {actual:#x}")]
    InvalidMagic { expected: u64, actual: u64 },

    #[error(
        "Protocol version mismatch: server version {server} is incompatible with client range {min}-{max}"
    )]
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

    #[error("Connection timeout")]
    ConnectionTimeout,

    #[error("Pool timeout waiting for available connection")]
    PoolTimeout,

    #[error("Invalid UTF-8 in string data: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
}

impl ProtocolError {
    /// Create an IO error with context
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

/// Extension trait for adding context to IO errors
pub trait IoErrorContext<T> {
    fn io_context(self, context: impl Into<String>) -> Result<T, ProtocolError>;
}

impl<T> IoErrorContext<T> for Result<T, std::io::Error> {
    fn io_context(self, context: impl Into<String>) -> Result<T, ProtocolError> {
        self.map_err(|e| ProtocolError::io(context, e))
    }
}

impl<T> IoErrorContext<T> for Result<T, ProtocolError> {
    fn io_context(self, context: impl Into<String>) -> Result<T, ProtocolError> {
        self.map_err(|e| match e {
            ProtocolError::Io {
                source,
                context: inner_context,
            } => ProtocolError::Io {
                context: format!("{}: {}", context.into(), inner_context),
                source,
            },
            other => other,
        })
    }
}
