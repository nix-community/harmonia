use harmonia_store_remote::{
    error::ProtocolError,
    protocol::{StorePath, ValidPathInfo},
    server::RequestHandler,
};
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

    /// Parse a store path and validate it belongs to our store
    fn parse_store_path(&self, path_str: &str) -> Result<PathBuf, ProtocolError> {
        let path = PathBuf::from(path_str);

        // Canonicalize and check it's under the store directory
        if !path.starts_with(&self.store_dir) {
            return Err(ProtocolError::DaemonError {
                message: format!("path '{path_str}' is not in the Nix store"),
            });
        }

        Ok(path)
    }
}

impl RequestHandler for LocalStoreHandler {
    async fn handle_query_path_info(
        &self,
        path: &StorePath,
    ) -> Result<Option<ValidPathInfo>, ProtocolError> {
        let path_str = path.to_string();
        let store_path = self.parse_store_path(&path_str)?;

        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let db = db.blocking_lock();
            db.query_path_info(&store_path)
        })
        .await
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Task join error: {e}"),
        })?
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
        let store_dir = self.store_dir.clone();
        tokio::task::spawn_blocking(move || {
            let db = db.blocking_lock();
            db.query_path_from_hash_part(&store_dir, &hash_str)
        })
        .await
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Task join error: {e}"),
        })?
    }

    async fn handle_is_valid_path(&self, path: &StorePath) -> Result<bool, ProtocolError> {
        let path_str = path.to_string();
        let store_path = self.parse_store_path(&path_str)?;

        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let db = db.blocking_lock();
            db.is_valid_path(&store_path)
        })
        .await
        .map_err(|e| ProtocolError::DaemonError {
            message: format!("Task join error: {e}"),
        })?
    }
}
