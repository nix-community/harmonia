use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Server error: {0}")]
    Server(#[from] ServerError),

    #[error("Store error: {0}")]
    Store(#[from] StoreError),

    #[error("NAR processing error: {0}")]
    Nar(#[from] NarError),

    #[error("Signing error: {0}")]
    Signing(#[from] SigningError),

    #[error("NARInfo error: {0}")]
    NarInfo(#[from] NarInfoError),

    #[error("Build log error: {0}")]
    BuildLog(#[from] BuildLogError),

    #[error("File serving error: {0}")]
    Serve(#[from] ServeError),
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse TOML: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("Invalid signing key: {reason}")]
    InvalidSigningKey { reason: String },

    #[error("Invalid configuration: {reason}")]
    Invalid { reason: String },
}

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("TLS setup failed: {reason}")]
    TlsSetup { reason: String },

    #[error("Server startup failed: {reason}")]
    Startup { reason: String },
}

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Failed to query path for hash {hash}: {reason}")]
    PathQuery { hash: String, reason: String },

    #[error("Store operation failed: {reason}")]
    Operation { reason: String },

    #[error("Remote store error: {0}")]
    Remote(#[from] harmonia_store_remote::ProtocolError),
}

#[derive(Error, Debug)]
pub enum NarError {
    #[error("Failed to read NAR file {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read symlink target for {path}: {source}")]
    SymlinkRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to stream NAR: {reason}")]
    Streaming { reason: String },

    #[error("Channel send failed: {reason}")]
    ChannelSend { reason: String },
}

#[derive(Error, Debug)]
pub enum SigningError {
    #[error("Failed to parse signing key: {reason}")]
    ParseKey { reason: String },

    #[error("Base64 decode error: {0}")]
    Base64Decode(#[from] base64::DecodeError),

    #[error("Invalid signature: {reason}")]
    InvalidSignature { reason: String },
}

#[derive(Error, Debug)]
pub enum NarInfoError {
    #[error("Failed to query path info: {reason}")]
    QueryFailed { reason: String },
}

#[derive(Error, Debug)]
pub enum BuildLogError {
    #[error("Failed to query derivation path: {reason}")]
    QueryFailed { reason: String },
}

#[derive(Error, Debug)]
pub enum ServeError {
    #[error("Failed to serve file: {source}")]
    ServeFailed {
        #[source]
        source: std::io::Error,
    },

    #[error("Access denied: {path}")]
    AccessDenied { path: String },
}

pub type Result<T> = std::result::Result<T, CacheError>;

/// Extension trait for adding context to IO errors
pub trait IoErrorContext<T> {
    fn io_context(self, context: impl Into<String>) -> Result<T>;
}

impl<T> IoErrorContext<T> for std::result::Result<T, std::io::Error> {
    fn io_context(self, context: impl Into<String>) -> Result<T> {
        self.map_err(|e| CacheError::Io {
            context: context.into(),
            source: e,
        })
    }
}
