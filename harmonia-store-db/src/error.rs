// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Error types for store database operations.

use std::path::PathBuf;

use thiserror::Error;

/// Result type for store database operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during store database operations.
#[derive(Error, Debug)]
pub enum Error {
    /// SQLite error
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Failed to open database with context
    #[error("Failed to open database at '{path}': {source}")]
    DatabaseOpen {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },

    /// Invalid store path
    #[error("Invalid store path: {0}")]
    InvalidStorePath(String),

    /// Path not found in database
    #[error("Path not found: {0}")]
    PathNotFound(String),

    /// Database file not found
    #[error("Database not found at: {0}")]
    DatabaseNotFound(PathBuf),

    /// Schema version mismatch
    #[error("Schema version mismatch: expected {expected}, found {found}")]
    SchemaVersionMismatch { expected: i32, found: i32 },

    /// Parse error for signatures
    #[error("Invalid signature format: {0}")]
    InvalidSignature(String),

    /// Parse error for content address
    #[error("Invalid content address: {0}")]
    InvalidContentAddress(String),
}
