// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Database connection management.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use tracing::debug;

use crate::error::{Error, Result};
use crate::schema::{CA_SCHEMA_SQL, SCHEMA_SQL};

/// Database open mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// Read-only access (for production use with system database)
    ReadOnly,
    /// Read-write access (for testing or local store management)
    ReadWrite,
    /// Create new database if it doesn't exist
    Create,
}

/// SQLite database connection for Nix store metadata.
pub struct StoreDb {
    pub(crate) conn: Connection,
}

impl StoreDb {
    /// Open the system Nix store database at `/nix/var/nix/db/db.sqlite`.
    ///
    /// Opens in read-only mode with immutable flag for safety.
    pub fn open_system() -> Result<Self> {
        Self::open_system_at("/nix/var/nix/db/db.sqlite")
    }

    /// Open a system database at a custom path (read-only).
    pub fn open_system_at<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(Error::DatabaseNotFound(path.to_owned()));
        }

        // Use URI with immutable flag for read-only access
        let uri = format!("file:{}?immutable=1", path.display());
        let conn = Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| Error::DatabaseOpen {
            path: path.to_owned(),
            source: e,
        })?;

        debug!("Opened system database at {}", path.display());
        Ok(Self { conn })
    }

    /// Open or create a database at a custom path.
    pub fn open<P: AsRef<Path>>(path: P, mode: OpenMode) -> Result<Self> {
        let path = path.as_ref();
        let flags = match mode {
            OpenMode::ReadOnly => {
                if !path.exists() {
                    return Err(Error::DatabaseNotFound(path.to_owned()));
                }
                OpenFlags::SQLITE_OPEN_READ_ONLY
            }
            OpenMode::ReadWrite => {
                if !path.exists() {
                    return Err(Error::DatabaseNotFound(path.to_owned()));
                }
                OpenFlags::SQLITE_OPEN_READ_WRITE
            }
            OpenMode::Create => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        };

        let conn = Connection::open_with_flags(path, flags).map_err(|e| Error::DatabaseOpen {
            path: path.to_owned(),
            source: e,
        })?;
        let db = Self { conn };

        if mode == OpenMode::Create {
            db.configure_pragmas()?;
        }

        debug!("Opened database at {} ({:?})", path.display(), mode);
        Ok(db)
    }

    /// Create an in-memory database (for testing).
    ///
    /// The database is initialized with the full schema.
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.configure_pragmas()?;
        db.create_schema()?;
        debug!("Created in-memory database");
        Ok(db)
    }

    /// Configure SQLite pragmas for optimal performance.
    fn configure_pragmas(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA temp_store = MEMORY;
            "#,
        )?;
        Ok(())
    }

    /// Create the database schema (core + CA tables).
    pub fn create_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA_SQL)?;
        self.conn.execute_batch(CA_SCHEMA_SQL)?;
        debug!("Created database schema");
        Ok(())
    }

    /// Get raw connection (for advanced usage).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Get mutable raw connection (for transactions).
    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Check if the database has the expected schema tables.
    pub fn has_schema(&self) -> Result<bool> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ValidPaths'",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Check if the database has CA-derivations tables.
    pub fn has_ca_schema(&self) -> Result<bool> {
        let count: i32 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='Realisations'",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }
}
