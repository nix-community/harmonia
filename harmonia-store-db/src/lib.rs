// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! SQLite database interface for Nix store metadata.
//!
//! This crate provides read and write access to the Nix store's SQLite database,
//! enabling queries for store path metadata, references, and derivation outputs.
//!
//! **Architecture**: This is the Database Layer in Harmonia's store architecture.
//!
//! # Key Features
//!
//! - Full schema support (ValidPaths, Refs, DerivationOutputs, Realisations)
//! - Read-only system database access
//! - In-memory database for testing
//! - Write operations for testing and local store management
//!
//! # Example
//!
//! ```ignore
//! use harmonia_store_db::{StoreDb, OpenMode};
//!
//! // Open system database (read-only)
//! let db = StoreDb::open_system()?;
//!
//! // Query a path
//! if let Some(info) = db.query_path_info("/nix/store/...")? {
//!     println!("NAR size: {}", info.nar_size.unwrap_or(0));
//! }
//! ```

mod connection;
mod error;
mod query;
mod schema;
mod types;
mod write;

pub use connection::{OpenMode, StoreDb};
pub use error::{Error, Result};
pub use schema::SCHEMA_VERSION;
pub use types::*;
pub use write::*;
