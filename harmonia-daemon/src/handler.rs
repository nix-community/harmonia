// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Request handler for the local store daemon.
//!
//! This module provides the `LocalStoreHandler` which implements the daemon
//! protocol by querying the Nix store database via `harmonia-store-db`.

use std::collections::BTreeSet;
use std::future::ready;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use harmonia_protocol::daemon::{
    DaemonError as ProtocolError, DaemonResult, DaemonStore, FutureResultExt, HandshakeDaemonStore,
    ResultLog, TrustLevel,
};
use harmonia_protocol::valid_path_info::UnkeyedValidPathInfo;
use harmonia_store_core::store_path::{StoreDir, StorePath, StorePathHash};
use harmonia_store_db::StoreDb;
use harmonia_utils_hash::{Hash, NarHash, fmt::Any};

use crate::error::DaemonError;

/// A local store handler that reads from the Nix store database.
#[derive(Clone)]
pub struct LocalStoreHandler {
    store_dir: StoreDir,
    db: Arc<Mutex<StoreDb>>,
}

impl LocalStoreHandler {
    /// Create a new handler with the given store directory and database path.
    pub async fn new(store_dir: StoreDir, db_path: PathBuf) -> Result<Self, DaemonError> {
        log::debug!("Opening database at {}", db_path.display());
        let db = StoreDb::open(&db_path, harmonia_store_db::OpenMode::ReadOnly).map_err(|e| {
            DaemonError::Database(format!("Failed to open {}: {e}", db_path.display()))
        })?;
        Ok(Self {
            store_dir,
            db: Arc::new(Mutex::new(db)),
        })
    }

    /// Convert a harmonia_store_db::ValidPathInfo to the protocol UnkeyedValidPathInfo.
    fn to_protocol_path_info(
        info: harmonia_store_db::ValidPathInfo,
        store_dir: StoreDir,
    ) -> Result<UnkeyedValidPathInfo, ProtocolError> {
        // Parse the hash from database format (e.g., "sha256:...")
        let hash_any = info.hash.parse::<Any<Hash>>().map_err(|e| {
            ProtocolError::custom(format!("Failed to parse hash '{}': {e}", info.hash))
        })?;
        let nar_hash = NarHash::try_from(hash_any.into_hash()).map_err(|e| {
            ProtocolError::custom(format!("Failed to convert hash '{}': {e}", info.hash))
        })?;

        // Convert references from String to StorePath
        let references = info
            .references
            .iter()
            .filter_map(|path| {
                // References are stored as full paths, extract just the name
                let base_name = std::path::Path::new(path)
                    .file_name()
                    .and_then(|n| n.to_str())?;
                StorePath::from_base_path(base_name).ok()
            })
            .collect();

        // Convert deriver
        let deriver = info.deriver.as_ref().and_then(|d| {
            let base_name = std::path::Path::new(d)
                .file_name()
                .and_then(|n| n.to_str())?;
            StorePath::from_base_path(base_name).ok()
        });

        // Convert registration time
        let registration_time = info
            .registration_time
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Parse signatures
        let signatures = info
            .sigs
            .map(|s| {
                s.split_whitespace()
                    .filter_map(|sig| sig.parse().ok())
                    .collect()
            })
            .unwrap_or_default();

        // Parse content address
        let ca = info.ca.and_then(|s| s.parse().ok());

        Ok(UnkeyedValidPathInfo {
            deriver,
            nar_hash,
            references,
            registration_time,
            nar_size: info.nar_size.unwrap_or(0),
            ultimate: info.ultimate,
            signatures,
            ca,
            store_dir,
        })
    }
}

impl HandshakeDaemonStore for LocalStoreHandler {
    type Store = Self;

    fn handshake(self) -> impl ResultLog<Output = DaemonResult<Self::Store>> + Send {
        ready(Ok(self)).empty_logs()
    }
}

impl DaemonStore for LocalStoreHandler {
    fn trust_level(&self) -> TrustLevel {
        TrustLevel::Trusted
    }

    fn is_valid_path<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<bool>> + Send + 'a {
        async move {
            let full_path = format!("{}/{}", self.store_dir, path);
            let db = self.db.clone();
            tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.is_valid_path(&full_path)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))
        }
        .empty_logs()
    }

    fn query_path_info<'a>(
        &'a mut self,
        path: &'a StorePath,
    ) -> impl ResultLog<Output = DaemonResult<Option<UnkeyedValidPathInfo>>> + Send + 'a {
        async move {
            let full_path = format!("{}/{}", self.store_dir, path);
            let db = self.db.clone();
            let store_dir = self.store_dir.clone();
            let result = tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.query_path_info(&full_path)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

            result
                .map(|info| Self::to_protocol_path_info(info, store_dir))
                .transpose()
        }
        .empty_logs()
    }

    fn query_path_from_hash_part<'a>(
        &'a mut self,
        hash: &'a StorePathHash,
    ) -> impl ResultLog<Output = DaemonResult<Option<StorePath>>> + Send + 'a {
        async move {
            let hash_str = hash.to_string();
            let db = self.db.clone();
            let store_dir = self.store_dir.to_str().to_string();
            let result = tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.query_path_from_hash_part(&store_dir, &hash_str)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

            Ok(result.and_then(|path| {
                let base_name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|n| n.to_str())?;
                StorePath::from_base_path(base_name).ok()
            }))
        }
        .empty_logs()
    }

    fn query_valid_paths<'a>(
        &'a mut self,
        paths: &'a harmonia_store_core::store_path::StorePathSet,
        _substitute: bool,
    ) -> impl ResultLog<Output = DaemonResult<harmonia_store_core::store_path::StorePathSet>> + Send + 'a
    {
        async move {
            let mut valid = BTreeSet::new();
            for path in paths {
                let full_path = format!("{}/{}", self.store_dir, path);
                let db = self.db.clone();
                let is_valid = tokio::task::spawn_blocking(move || {
                    let db = db.blocking_lock();
                    db.is_valid_path(&full_path)
                })
                .await
                .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
                .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

                if is_valid {
                    valid.insert(path.clone());
                }
            }
            Ok(valid)
        }
        .empty_logs()
    }

    fn shutdown(&mut self) -> impl std::future::Future<Output = DaemonResult<()>> + Send + '_ {
        ready(Ok(()))
    }
}
