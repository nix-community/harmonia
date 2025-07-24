use harmonia_store_remote::client::DaemonClient;
use harmonia_store_remote::protocol::StorePath;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;
use tokio::sync::Mutex;

#[derive(Default, Debug)]
pub struct Store {
    virtual_store: Vec<u8>,
    real_store: Option<Vec<u8>>,
    pub daemon: Mutex<Option<DaemonClient>>,
}

impl Store {
    pub fn new(virtual_store: Vec<u8>, real_store: Option<Vec<u8>>) -> Self {
        Self {
            virtual_store,
            real_store,
            daemon: Mutex::new(None),
        }
    }
    pub fn get_real_path(&self, store_path: &StorePath) -> PathBuf {
        let virtual_path = Path::new(OsStr::from_bytes(store_path.as_bytes()));
        if self.real_store.is_some()
            && virtual_path.starts_with(OsStr::from_bytes(&self.virtual_store))
        {
            return self.real_store().join(
                virtual_path
                    .strip_prefix(OsStr::from_bytes(&self.virtual_store))
                    .unwrap(),
            );
        }
        PathBuf::from(virtual_path)
    }

    pub fn real_store(&self) -> &Path {
        Path::new(OsStr::from_bytes(
            self.real_store.as_ref().unwrap_or(&self.virtual_store),
        ))
    }

    pub fn virtual_store(&self) -> &[u8] {
        &self.virtual_store
    }

    pub async fn get_daemon(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<DaemonClient>>, anyhow::Error> {
        use anyhow::Context;

        let mut daemon_guard = self.daemon.lock().await;

        // Connect to daemon if not already connected
        if daemon_guard.is_none() {
            let client =
                DaemonClient::connect(std::path::Path::new("/nix/var/nix/daemon-socket/socket"))
                    .await
                    .context("Failed to connect to nix daemon")?;
            *daemon_guard = Some(client);
        }

        Ok(daemon_guard)
    }
}
