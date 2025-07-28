use harmonia_store_remote::{
    error::ProtocolError,
    protocol::types::{
        DerivedPath, DrvOutputId, Missing, Realisation, SubstitutablePathInfo,
        SubstitutablePathInfos,
    },
    protocol::{StorePath, ValidPathInfo},
    server::RequestHandler,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::DaemonError;
use crate::sqlite::StoreDb;

/// A local store handler that reads from the Nix store database
#[derive(Clone)]
pub struct LocalStoreHandler {
    store_dir: PathBuf,
    db: Arc<Mutex<StoreDb>>,
}

impl LocalStoreHandler {
    pub async fn new(store_dir: PathBuf, db_path: PathBuf) -> Result<Self, DaemonError> {
        let db = StoreDb::open(&db_path)?;
        Ok(Self {
            store_dir,
            db: Arc::new(Mutex::new(db)),
        })
    }

    /// Extract and validate a store path from bytes, returning it as a string
    fn extract_valid_store_path<'a>(
        path: &'a StorePath,
        store_dir: &PathBuf,
    ) -> Result<&'a str, ProtocolError> {
        let path_str =
            std::str::from_utf8(path.as_bytes()).map_err(|_| ProtocolError::DaemonError {
                message: "Store path is not valid UTF-8".to_string(),
            })?;

        // Check it starts with the store directory
        let store_dir_str = store_dir
            .to_str()
            .ok_or_else(|| ProtocolError::DaemonError {
                message: "Store directory path is not valid UTF-8".to_string(),
            })?;

        if !path_str.starts_with(store_dir_str) {
            return Err(ProtocolError::DaemonError {
                message: format!("path '{path_str}' is not in the Nix store"),
            });
        }

        Ok(path_str)
    }

    /// Execute a database operation asynchronously
    async fn db_operation<T, F>(&self, f: F) -> Result<T, ProtocolError>
    where
        T: Send + 'static,
        F: FnOnce(&StoreDb) -> Result<T, ProtocolError> + Send + 'static,
    {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let db = db.blocking_lock();
            f(&db)
        })
        .await
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Task join error: {e}"),
        })?
    }
}

impl RequestHandler for LocalStoreHandler {
    async fn handle_query_path_info(
        &self,
        path: StorePath,
    ) -> Result<Option<ValidPathInfo>, ProtocolError> {
        let store_dir = self.store_dir.clone();
        self.db_operation(move |db| {
            let path_str = Self::extract_valid_store_path(&path, &store_dir)?;
            db.query_path_info(path_str)
        })
        .await
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

        let store_dir = self.store_dir.clone();
        self.db_operation(move |db| db.query_path_from_hash_part(&store_dir, &hash_str))
            .await
    }

    async fn handle_is_valid_path(&self, path: StorePath) -> Result<bool, ProtocolError> {
        let store_dir = self.store_dir.clone();
        self.db_operation(move |db| {
            let path_str = Self::extract_valid_store_path(&path, &store_dir)?;
            db.is_valid_path(path_str)
        })
        .await
    }

    async fn handle_query_all_valid_paths(&self) -> Result<BTreeSet<StorePath>, ProtocolError> {
        self.db_operation(|db| db.query_all_valid_paths()).await
    }

    async fn handle_query_valid_paths(
        &self,
        paths: BTreeSet<StorePath>,
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        let mut valid_paths = BTreeSet::new();
        for path in paths {
            if self.handle_is_valid_path(path.clone()).await? {
                valid_paths.insert(path);
            }
        }
        Ok(valid_paths)
    }

    async fn handle_query_substitutable_paths(
        &self,
        _paths: BTreeSet<StorePath>,
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        // TODO: When implementing remote store support, this should:
        // 1. Check configured substituters (binary caches)
        // 2. Query each substituter for availability of these paths
        // 3. Return paths that are available in at least one substituter
        // For now, local store doesn't have substitutes
        Ok(BTreeSet::new())
    }

    async fn handle_has_substitutes(&self, _path: StorePath) -> Result<bool, ProtocolError> {
        // TODO: When implementing remote store support, this should:
        // 1. Check if the path is available in any configured substituter
        // 2. Could be optimized to stop at first positive result
        // For now, local store doesn't have substitutes
        Ok(false)
    }

    async fn handle_query_substitutable_path_info(
        &self,
        _path: StorePath,
    ) -> Result<Option<SubstitutablePathInfo>, ProtocolError> {
        // TODO: When implementing remote store support, this should:
        // 1. Query configured substituters for path info
        // 2. Return info including download size, NAR size, references
        // 3. Could cache results to avoid repeated queries
        // For now, local store doesn't have substitutes
        Ok(None)
    }

    async fn handle_query_substitutable_path_infos(
        &self,
        _paths: BTreeSet<StorePath>,
    ) -> Result<SubstitutablePathInfos, ProtocolError> {
        // TODO: When implementing remote store support, this should:
        // 1. Batch query substituters for multiple paths efficiently
        // 2. Merge results from multiple substituters
        // 3. Handle partial failures gracefully
        // For now, local store doesn't have substitutes
        Ok(BTreeMap::new())
    }

    async fn handle_query_referrers(
        &self,
        path: StorePath,
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        let store_dir = self.store_dir.clone();
        self.db_operation(move |db| {
            let path_str = Self::extract_valid_store_path(&path, &store_dir)?;
            db.query_referrers(path_str)
        })
        .await
    }

    async fn handle_query_valid_derivers(
        &self,
        path: StorePath,
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        let store_dir = self.store_dir.clone();
        self.db_operation(move |db| {
            let path_str = Self::extract_valid_store_path(&path, &store_dir)?;
            db.query_valid_derivers(path_str)
        })
        .await
    }

    async fn handle_query_derivation_outputs(
        &self,
        drv_path: StorePath,
    ) -> Result<BTreeSet<StorePath>, ProtocolError> {
        let store_dir = self.store_dir.clone();
        self.db_operation(move |db| {
            let path_str = Self::extract_valid_store_path(&drv_path, &store_dir)?;
            db.query_derivation_outputs(path_str)
        })
        .await
    }

    async fn handle_query_derivation_output_names(
        &self,
        drv_path: StorePath,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let store_dir = self.store_dir.clone();
        self.db_operation(move |db| {
            let path_str = Self::extract_valid_store_path(&drv_path, &store_dir)?;
            db.query_derivation_output_names(path_str)
        })
        .await
    }

    async fn handle_query_derivation_output_map(
        &self,
        drv_path: StorePath,
    ) -> Result<BTreeMap<String, Option<StorePath>>, ProtocolError> {
        let store_dir = self.store_dir.clone();
        self.db_operation(move |db| {
            let path_str = Self::extract_valid_store_path(&drv_path, &store_dir)?;
            db.query_derivation_output_map(path_str)
        })
        .await
    }

    async fn handle_query_missing(
        &self,
        targets: Vec<DerivedPath>,
    ) -> Result<Missing, ProtocolError> {
        // For local store, we don't need to build or substitute anything
        // Just check which paths are missing
        let mut unknown = BTreeSet::new();
        for target in targets {
            // Extract the store path from DerivedPath
            let store_path = match target {
                DerivedPath::Opaque(path) => path,
                DerivedPath::Built(path, _) => path,
            };
            if !self.handle_is_valid_path(store_path.clone()).await? {
                unknown.insert(store_path);
            }
        }

        Ok(Missing {
            will_build: BTreeSet::new(),
            will_substitute: BTreeSet::new(),
            unknown_paths: unknown,
            download_size: 0,
            nar_size: 0,
        })
    }

    async fn handle_query_realisation(
        &self,
        id: DrvOutputId,
    ) -> Result<Option<Realisation>, ProtocolError> {
        let drv_hash = id.drv_hash;
        let output_name =
            String::from_utf8(id.output_name).map_err(|_| ProtocolError::DaemonError {
                message: "Invalid UTF-8 in output name".to_string(),
            })?;

        self.db_operation(move |db| db.query_realisation(&drv_hash, &output_name))
            .await
    }

    async fn handle_query_failed_paths(&self) -> Result<BTreeSet<StorePath>, ProtocolError> {
        // TODO: Implement failed paths tracking
        // Nix tracks failed builds in a FailedPaths table in the SQLite database
        // Query: SELECT path FROM FailedPaths
        // This helps avoid rebuilding known-failing derivations
        Ok(BTreeSet::new())
    }

    async fn handle_clear_failed_paths(
        &self,
        _paths: BTreeSet<StorePath>,
    ) -> Result<(), ProtocolError> {
        // TODO: Implement failed paths clearing
        // Should remove entries from the FailedPaths table
        // Query: DELETE FROM FailedPaths WHERE path IN (?)
        // This allows retrying builds that previously failed
        Ok(())
    }
}
