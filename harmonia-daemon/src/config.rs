use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{DaemonError, IoContext};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Path to bind the daemon socket
    pub socket_path: PathBuf,

    /// Path to the Nix store directory
    pub store_dir: PathBuf,

    /// Path to the Nix database
    pub db_path: PathBuf,

    /// Number of worker threads
    pub workers: Option<usize>,

    /// Log level
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/run/harmonia-daemon.sock"),
            store_dir: PathBuf::from("/nix/store"),
            db_path: PathBuf::from("/nix/var/nix/db/db.sqlite"),
            workers: None,
            log_level: "info".to_string(),
        }
    }
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self, DaemonError> {
        let contents = std::fs::read_to_string(path)
            .io_context(|| format!("Failed to read config file at {}", path.display()))?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }
}
