use crate::error::{Result, StoreError};
use harmonia_store_core::store_path::{StoreDir, StorePath};
use harmonia_store_remote::pool::{ConnectionPool, PoolConfig, PooledConnectionGuard};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Store {
    virtual_store: Vec<u8>,
    real_store: Option<Vec<u8>>,
    pool: ConnectionPool,
}

impl Store {
    pub fn new(
        virtual_store: Vec<u8>,
        real_store: Option<Vec<u8>>,
        daemon_socket: PathBuf,
        pool_config: PoolConfig,
    ) -> Self {
        // Parse store_dir from virtual_store bytes
        let store_dir_str = std::str::from_utf8(&virtual_store).unwrap_or("/nix/store");
        let store_dir = StoreDir::new(store_dir_str).unwrap_or_default();

        let pool = ConnectionPool::with_store_dir(&daemon_socket, store_dir, pool_config);

        Self {
            virtual_store,
            real_store,
            pool,
        }
    }

    pub fn get_real_path(&self, store_path: &StorePath) -> PathBuf {
        // StorePath is now just "hash-name", construct full path
        let virtual_store_path = Path::new(OsStr::from_bytes(&self.virtual_store));
        let full_virtual_path = virtual_store_path.join(store_path.to_string());

        if self.real_store.is_some() {
            // Map from virtual store to real store
            return self.real_store().join(store_path.to_string());
        }
        full_virtual_path
    }

    pub fn real_store(&self) -> &Path {
        Path::new(OsStr::from_bytes(
            self.real_store.as_ref().unwrap_or(&self.virtual_store),
        ))
    }

    pub fn virtual_store(&self) -> &[u8] {
        &self.virtual_store
    }

    pub fn to_virtual_path(&self, store_path: &StorePath) -> StorePath {
        // StorePath is now just "hash-name", which is store-agnostic
        // No translation needed - just return the same StorePath
        store_path.clone()
    }

    pub async fn acquire(&self) -> Result<PooledConnectionGuard> {
        self.pool
            .acquire()
            .await
            .map_err(|e| StoreError::Remote(e).into())
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new(
            b"/nix/store".to_vec(),
            None,
            PathBuf::from("/nix/var/nix/daemon-socket/socket"),
            PoolConfig {
                max_size: 2, // Small pool for tests
                ..Default::default()
            },
        )
    }
}
