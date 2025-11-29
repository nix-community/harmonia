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

    #[error("Signing error: {0}")]
    Signing(#[from] harmonia_store_core::signature::ParseKeyError),

    #[error("Fingerprint error: {0}")]
    Fingerprint(#[from] harmonia_store_core::signature::FingerprintError),

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

    #[error("invalid signing key: {reason}")]
    InvalidSigningKey { reason: String },

    #[error("invalid configuration: {reason}")]
    Invalid { reason: String },
}

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("TLS setup failed: {reason}")]
    TlsSetup { reason: String },

    #[error("server startup failed: {reason}")]
    Startup { reason: String },
}

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("failed to query store path '{hash}': {reason}")]
    PathQuery { hash: String, reason: String },

    #[error("Daemon communication error: {0}")]
    Remote(#[from] harmonia_store_remote::DaemonError),
}

#[derive(Error, Debug)]
pub enum NarInfoError {
    #[error("Failed to query path info: {reason}")]
    QueryFailed { reason: String },

    #[error("Invalid UTF-8 in store directory: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),

    #[error("Invalid store directory: {0}")]
    InvalidStoreDir(String),
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
