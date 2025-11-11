use crate::error::{Result, StoreError};
use harmonia_store_remote_legacy::client::{DaemonClient, PoolConfig};
use harmonia_store_remote_legacy::protocol::StorePath;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Store {
    virtual_store: Vec<u8>,
    real_store: Option<Vec<u8>>,
    daemon_socket: PathBuf,
    pub daemon: Arc<Mutex<Option<DaemonClient>>>,
    pub pool_config: PoolConfig,
}

impl Store {
    pub fn new(
        virtual_store: Vec<u8>,
        real_store: Option<Vec<u8>>,
        daemon_socket: PathBuf,
        pool_config: PoolConfig,
    ) -> Self {
        Self {
            virtual_store,
            real_store,
            daemon_socket,
            daemon: Arc::new(Mutex::new(None)),
            pool_config,
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

    pub async fn get_daemon(&self) -> Result<tokio::sync::MutexGuard<'_, Option<DaemonClient>>> {
        let mut daemon_guard = self.daemon.lock().await;

        // Connect to daemon if not already connected
        if daemon_guard.is_none() {
            log::debug!("Connecting to daemon at {:?}", self.daemon_socket);
            match DaemonClient::connect_with_config(&self.daemon_socket, self.pool_config.clone())
                .await
            {
                Ok(client) => {
                    log::debug!("Successfully connected to daemon");
                    *daemon_guard = Some(client);
                }
                Err(e) => {
                    log::error!(
                        "Failed to connect to daemon at {:?}: {}",
                        self.daemon_socket,
                        e
                    );
                    return Err(StoreError::Remote(e).into());
                }
            }
        }

        Ok(daemon_guard)
    }
}

impl Default for Store {
    fn default() -> Self {
        Self {
            virtual_store: b"/nix/store".to_vec(),
            real_store: None,
            daemon_socket: PathBuf::from("/nix/var/nix/daemon-socket/socket"),
            daemon: Arc::new(Mutex::new(None)),
            pool_config: PoolConfig {
                max_size: 2, // Small pool for tests
                ..Default::default()
            },
        }
    }
}
