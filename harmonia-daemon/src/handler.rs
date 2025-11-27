// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Request handler for the local store daemon.
//!
//! This module provides the `LocalStoreHandler` which implements the daemon
//! protocol by querying the Nix store database via `harmonia-store-db`.

use harmonia_store_core::hash::{Hash, fmt::Any};
use harmonia_store_db::StoreDb;
use harmonia_store_remote_legacy::{
    error::ProtocolError,
    protocol::{StorePath, ValidPathInfo},
    server::RequestHandler,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::DaemonError;

/// A local store handler that reads from the Nix store database.
#[derive(Clone)]
pub struct LocalStoreHandler {
    store_dir: PathBuf,
    db: Arc<Mutex<StoreDb>>,
}

impl LocalStoreHandler {
    /// Create a new handler with the given store directory and database path.
    pub async fn new(store_dir: PathBuf, db_path: PathBuf) -> Result<Self, DaemonError> {
        let db = StoreDb::open(&db_path, harmonia_store_db::OpenMode::ReadOnly)
            .map_err(|e| DaemonError::Database(format!("{e}")))?;
        Ok(Self {
            store_dir,
            db: Arc::new(Mutex::new(db)),
        })
    }

    /// Parse a store path and validate it belongs to our store.
    fn parse_store_path(&self, path_str: &str) -> Result<PathBuf, ProtocolError> {
        let path = PathBuf::from(path_str);

        // Check it's under the store directory
        if !path.starts_with(&self.store_dir) {
            return Err(ProtocolError::DaemonError {
                message: format!("path '{path_str}' is not in the Nix store"),
            });
        }

        Ok(path)
    }

    /// Convert a full path string to a legacy StorePath.
    fn to_legacy_store_path(full_path: &str) -> Result<StorePath, ProtocolError> {
        // Find the last '/' to get just the "hash-name" part
        let base_name = full_path
            .rsplit('/')
            .next()
            .ok_or_else(|| ProtocolError::DaemonError {
                message: format!("Invalid store path format: {full_path}"),
            })?;

        StorePath::from_bytes(base_name.as_bytes()).map_err(|e| ProtocolError::DaemonError {
            message: format!("Failed to parse store path '{base_name}': {e}"),
        })
    }

    /// Convert a harmonia_store_db::ValidPathInfo to the legacy protocol ValidPathInfo.
    fn to_legacy_path_info(
        info: harmonia_store_db::ValidPathInfo,
    ) -> Result<ValidPathInfo, ProtocolError> {
        // Parse the hash from database format (e.g., "sha256:...")
        let parsed_hash = info
            .hash
            .parse::<Any<Hash>>()
            .map_err(|e| ProtocolError::DaemonError {
                message: format!("Failed to parse hash '{}': {e}", info.hash),
            })?
            .into_hash();

        // Convert references from String to StorePath
        let references = info
            .references
            .iter()
            .map(|path| Self::to_legacy_store_path(path))
            .collect::<Result<_, _>>()?;

        // Convert deriver
        let deriver = info
            .deriver
            .as_ref()
            .map(|d| Self::to_legacy_store_path(d))
            .transpose()?;

        // Convert registration time
        let registration_time = info
            .registration_time
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Ok(ValidPathInfo {
            deriver,
            hash: parsed_hash,
            references,
            registration_time,
            nar_size: info.nar_size.unwrap_or(0),
            ultimate: info.ultimate,
            signatures: info
                .sigs
                .map(|s| {
                    s.split_whitespace()
                        .map(|sig| sig.as_bytes().to_vec())
                        .collect()
                })
                .unwrap_or_default(),
            content_address: info.ca.map(|s| s.into_bytes()),
        })
    }
}

impl RequestHandler for LocalStoreHandler {
    async fn handle_query_path_info(
        &self,
        path: &StorePath,
    ) -> Result<Option<ValidPathInfo>, ProtocolError> {
        // Construct full path from store directory + StorePath
        let full_path = format!("{}/{}", self.store_dir.display(), path);
        let _ = self.parse_store_path(&full_path)?;

        let db = self.db.clone();
        let result = tokio::task::spawn_blocking(move || {
            let db = db.blocking_lock();
            db.query_path_info(&full_path)
        })
        .await
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Task join error: {e}"),
        })?
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Database error: {e}"),
        })?;

        result.map(Self::to_legacy_path_info).transpose()
    }

    async fn handle_query_path_from_hash_part(
        &self,
        hash: &[u8],
    ) -> Result<Option<StorePath>, ProtocolError> {
        let hash_str = std::str::from_utf8(hash)
            .map_err(ProtocolError::InvalidUtf8)?
            .to_string();

        // Hash part must be exactly 32 characters
        if hash_str.len() != 32 {
            return Err(ProtocolError::DaemonError {
                message: "invalid hash part length".to_string(),
            });
        }

        let db = self.db.clone();
        let store_dir = self.store_dir.to_string_lossy().to_string();
        let result = tokio::task::spawn_blocking(move || {
            let db = db.blocking_lock();
            db.query_path_from_hash_part(&store_dir, &hash_str)
        })
        .await
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Task join error: {e}"),
        })?
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Database error: {e}"),
        })?;

        result
            .map(|path| Self::to_legacy_store_path(&path))
            .transpose()
    }

    async fn handle_is_valid_path(&self, path: &StorePath) -> Result<bool, ProtocolError> {
        // Construct full path from store directory + StorePath
        let full_path = format!("{}/{}", self.store_dir.display(), path);
        let _ = self.parse_store_path(&full_path)?;

        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let db = db.blocking_lock();
            db.is_valid_path(&full_path)
        })
        .await
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Task join error: {e}"),
        })?
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Database error: {e}"),
        })
    }
}
