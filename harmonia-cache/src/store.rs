use crate::daemon::DaemonConnection;
use core::str;
use std::path::Path;
use std::path::PathBuf;
use tokio::sync::Mutex;

#[derive(Default, Debug)]
pub struct Store {
    virtual_store: String,
    real_store: Option<String>,
    pub daemon: Mutex<DaemonConnection>,
}

impl Store {
    pub fn new(virtual_store: String, real_store: Option<String>) -> Self {
        Self {
            virtual_store,
            real_store,
            daemon: Default::default(),
        }
    }
    pub fn get_real_path(&self, virtual_path: &Path) -> PathBuf {
        if self.real_store.is_some() && virtual_path.starts_with(&self.virtual_store) {
            return self
                .real_store()
                .join(virtual_path.strip_prefix(&self.virtual_store).unwrap());
        }
        PathBuf::from(virtual_path)
    }

    pub fn real_store(&self) -> &Path {
        Path::new(self.real_store.as_ref().unwrap_or(&self.virtual_store))
    }

    pub fn virtual_store(&self) -> &str {
        &self.virtual_store
    }
}
