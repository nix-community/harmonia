use crate::error::{CacheError, ConfigError, Result};
use crate::signing::parse_secret_key;
use crate::store::Store;
use serde::Deserialize;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

fn default_bind() -> String {
    "[::]:5000".into()
}

fn default_workers() -> usize {
    4
}

fn default_connection_rate() -> usize {
    256
}

fn default_priority() -> usize {
    30
}

fn default_virtual_store() -> Vec<u8> {
    b"/nix/store".to_vec()
}

fn default_daemon_socket() -> PathBuf {
    PathBuf::from("/nix/var/nix/daemon-socket/socket")
}

#[derive(Debug)]
pub(crate) struct SigningKey {
    pub(crate) name: String,
    pub(crate) key: Vec<u8>,
}

// TODO(conni2461): users to restrict access
#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    #[serde(default = "default_bind")]
    pub(crate) bind: String,
    #[serde(default = "default_workers")]
    pub(crate) workers: usize,
    #[serde(default = "default_connection_rate")]
    pub(crate) max_connection_rate: usize,
    #[serde(default = "default_priority")]
    pub(crate) priority: usize,

    #[serde(default = "default_virtual_store")]
    pub(crate) virtual_nix_store: Vec<u8>,

    #[serde(default)]
    pub(crate) real_nix_store: Option<String>,

    #[serde(default)]
    pub(crate) sign_key_path: Option<String>,
    #[serde(default)]
    pub(crate) sign_key_paths: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) tls_cert_path: Option<String>,
    #[serde(default)]
    pub(crate) tls_key_path: Option<String>,

    #[serde(default = "default_daemon_socket")]
    pub(crate) daemon_socket: PathBuf,

    #[serde(skip, default)]
    pub(crate) secret_keys: Vec<SigningKey>,
    #[serde(skip)]
    pub(crate) store: Store,
}

impl Config {
    pub(crate) fn load(settings_file: &Path) -> Result<Config> {
        let contents = read_to_string(settings_file).map_err(|e| ConfigError::ReadFile {
            path: settings_file.display().to_string(),
            source: e,
        })?;
        toml::from_str(&contents).map_err(|e| CacheError::from(ConfigError::from(e)))
    }
}

pub(crate) fn load() -> Result<Config> {
    let mut settings = match std::env::var("CONFIG_FILE") {
        Err(_) => {
            if Path::new("settings.toml").exists() {
                Config::load(Path::new("settings.toml"))?
            } else {
                return Ok(Config::default());
            }
        }
        Ok(settings_file) => Config::load(Path::new(&settings_file))?,
    };

    if settings.workers == 0 {
        return Err(ConfigError::Invalid {
            reason: "workers must be greater than 0".to_string(),
        }
        .into());
    }

    if let Some(sign_key_path) = &settings.sign_key_path {
        log::warn!(
            "The sign_key_path configuration option is deprecated. Use sign_key_paths instead."
        );
        settings.sign_key_paths.push(PathBuf::from(sign_key_path));
    }
    if let Ok(sign_key_path) = std::env::var("SIGN_KEY_PATH") {
        log::warn!(
            "The SIGN_KEY_PATH environment variable is deprecated. Use SIGN_KEY_PATHS instead."
        );
        settings.sign_key_paths.push(PathBuf::from(sign_key_path));
    }
    if let Ok(sign_key_paths) = std::env::var("SIGN_KEY_PATHS") {
        for sign_key_path in sign_key_paths.split_whitespace() {
            settings.sign_key_paths.push(PathBuf::from(sign_key_path));
        }
    }
    for sign_key_path in &settings.sign_key_paths {
        settings
            .secret_keys
            .push(
                parse_secret_key(sign_key_path).map_err(|e| ConfigError::InvalidSigningKey {
                    reason: format!(
                        "Couldn't parse secret key from '{}': {}",
                        sign_key_path.display(),
                        e
                    ),
                })?,
            );
    }
    let store_dir = std::env::var_os("NIX_STORE_DIR")
        .map(|s| s.into_encoded_bytes())
        .unwrap_or_else(|| settings.virtual_nix_store.clone());
    settings.store = Store::new(
        store_dir,
        settings.real_nix_store.clone().map(|s| s.into_bytes()),
        settings.daemon_socket.clone(),
    );
    Ok(settings)
}
