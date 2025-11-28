// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DaemonError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Store database error: {0}")]
    StoreDb(#[from] harmonia_store_db::Error),

    #[error("IO error: {message}")]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Protocol error: {0}")]
    Protocol(#[from] harmonia_protocol::types::DaemonError),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("TOML parsing error: {0}")]
    Toml(#[from] toml::de::Error),
}

impl DaemonError {
    pub fn database(message: impl Into<String>) -> Self {
        Self::Database(message.into())
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }

    pub fn io(message: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            message: message.into(),
            source,
        }
    }
}

/// Helper trait for adding context to IO errors
pub trait IoContext<T> {
    fn io_context<F>(self, f: F) -> Result<T, DaemonError>
    where
        F: FnOnce() -> String;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn io_context<F>(self, f: F) -> Result<T, DaemonError>
    where
        F: FnOnce() -> String,
    {
        self.map_err(|e| DaemonError::io(f(), e))
    }
}

/// Helper trait for adding context to database errors
pub trait DbContext<T> {
    fn db_context<F>(self, f: F) -> Result<T, DaemonError>
    where
        F: FnOnce() -> String;
}

impl<T> DbContext<T> for Result<T, harmonia_store_db::Error> {
    fn db_context<F>(self, f: F) -> Result<T, DaemonError>
    where
        F: FnOnce() -> String,
    {
        self.map_err(|e| {
            let msg = f();
            DaemonError::Database(format!("{msg}: {e}"))
        })
    }
}
