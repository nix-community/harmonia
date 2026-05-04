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
use harmonia_store_core::realisation::{DrvOutput, UnkeyedRealisation};
use harmonia_store_core::store_path::{StoreDir, StorePath, StorePathHash};
use harmonia_store_db::StoreDb;

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
        tracing::debug!("Opening database at {}", db_path.display());
        let db = StoreDb::open(&db_path, harmonia_store_db::OpenMode::ReadOnly).map_err(|e| {
            DaemonError::Database(format!("Failed to open {}: {e}", db_path.display()))
        })?;
        Ok(Self {
            store_dir,
            db: Arc::new(Mutex::new(db)),
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
            let path = path.clone();
            let db = self.db.clone();
            let store_dir = self.store_dir.clone();
            tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.is_valid_path(&store_dir, &path)
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
            let path = path.clone();
            let db = self.db.clone();
            let store_dir = self.store_dir.clone();
            let result = tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.query_path_info(&store_dir, &path)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

            Ok(result.map(|info| info.info))
        }
        .empty_logs()
    }

    fn query_path_from_hash_part<'a>(
        &'a mut self,
        hash: &'a StorePathHash,
    ) -> impl ResultLog<Output = DaemonResult<Option<StorePath>>> + Send + 'a {
        async move {
            let hash = *hash;
            let db = self.db.clone();
            let store_dir = self.store_dir.clone();
            tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                db.query_path_from_hash_part(&store_dir, &hash)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))
        }
        .empty_logs()
    }

    fn query_realisation<'a>(
        &'a mut self,
        output_id: &'a DrvOutput,
    ) -> impl ResultLog<Output = DaemonResult<Option<UnkeyedRealisation>>> + Send + 'a {
        async move {
            let store_dir = self.store_dir.clone();
            let drv_path = output_id.drv_path.clone();
            let output_name = output_id.output_name.clone();
            let db = self.db.clone();
            let result = tokio::task::spawn_blocking(move || {
                let db = db.blocking_lock();
                if !db.has_ca_schema()? {
                    return Ok(None);
                }
                db.query_realisation(&store_dir, &drv_path, &output_name)
            })
            .await
            .map_err(|e| ProtocolError::custom(format!("Task join error: {e}")))?
            .map_err(|e| ProtocolError::custom(format!("Database error: {e}")))?;

            let Some(row) = result else {
                return Ok(None);
            };

            Ok(Some(row.realisation.value))
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
                let path_owned = path.clone();
                let db = self.db.clone();
                let store_dir = self.store_dir.clone();
                let is_valid = tokio::task::spawn_blocking(move || {
                    let db = db.blocking_lock();
                    db.is_valid_path(&store_dir, &path_owned)
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
