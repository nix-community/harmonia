use crate::error::{CacheError, ConfigError, Result};
use crate::store::Store;
use harmonia_store_core::signature::SecretKey;
use serde::Deserialize;
use std::ffi::OsStr;
use std::fs::read_to_string;
use std::os::unix::ffi::OsStrExt;
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

fn default_enable_compression() -> bool {
    false
}

/// zstd parameters applied to on-the-fly NAR encoding when the client sends
/// `Accept-Encoding: zstd`. Defaults are tuned for a substitution cache:
/// level 1 with long-distance matching beats the libzstd default (level 3)
/// on both ratio and throughput for typical NARs, and the window cap keeps
/// per-stream decoder memory bounded under parallel `nix copy`.
#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct ZstdConfig {
    #[serde(default = "ZstdConfig::default_level")]
    pub(crate) level: i32,
    #[serde(default = "ZstdConfig::default_long_distance")]
    pub(crate) long_distance_matching: bool,
    /// log2 of the match window. 0 = auto: with LDM, cap at 25 (32 MiB) so
    /// decoder memory stays bounded; without LDM, use the level default so
    /// the encoder doesn't allocate a large window it can't fill.
    #[serde(default)]
    pub(crate) window_log: u32,
}

impl ZstdConfig {
    fn default_level() -> i32 {
        1
    }
    fn default_long_distance() -> bool {
        true
    }
}

impl Default for ZstdConfig {
    fn default() -> Self {
        Self {
            level: Self::default_level(),
            long_distance_matching: Self::default_long_distance(),
            window_log: 0,
        }
    }
}

fn default_virtual_store() -> PathBuf {
    PathBuf::from("/nix/store")
}

/// Derive the location of `db.sqlite` from the on-disk store directory.
///
/// Nix lays out a store root as `<root>/store` and `<root>/var/nix/db/db.sqlite`,
/// so for both the default `/nix/store` and chroot stores we can find the
/// database by replacing the trailing `store` component.
fn derive_db_path(real_store: &Path) -> Option<PathBuf> {
    let root = real_store.parent()?;
    Some(root.join("var/nix/db/db.sqlite"))
}

// TODO(conni2461): users to restrict access
#[derive(Deserialize, Default)]
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
    pub(crate) virtual_nix_store: PathBuf,

    #[serde(default)]
    pub(crate) real_nix_store: Option<PathBuf>,

    #[serde(default = "default_enable_compression")]
    pub(crate) enable_compression: bool,

    #[serde(default)]
    pub(crate) zstd: ZstdConfig,

    #[serde(default)]
    pub(crate) sign_key_path: Option<String>,
    #[serde(default)]
    pub(crate) sign_key_paths: Vec<PathBuf>,
    #[serde(default)]
    pub(crate) tls_cert_path: Option<String>,
    #[serde(default)]
    pub(crate) tls_key_path: Option<String>,

    /// Path to the nix SQLite database. Derived from the store layout when unset.
    #[serde(default)]
    pub(crate) nix_db_path: Option<PathBuf>,

    #[serde(skip, default)]
    pub(crate) secret_keys: Vec<SecretKey>,
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
                Config::default()
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
        tracing::warn!(
            "The sign_key_path configuration option is deprecated. Use sign_key_paths instead."
        );
        settings.sign_key_paths.push(PathBuf::from(sign_key_path));
    }
    if let Ok(sign_key_path) = std::env::var("SIGN_KEY_PATH") {
        tracing::warn!(
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
        crate::tls::warn_insecure_permissions(sign_key_path);
        let key_content =
            read_to_string(sign_key_path).map_err(|e| ConfigError::InvalidSigningKey {
                reason: format!(
                    "Couldn't read secret key from '{}': {}",
                    sign_key_path.display(),
                    e
                ),
            })?;
        let key: SecretKey =
            key_content
                .trim()
                .parse()
                .map_err(|e| ConfigError::InvalidSigningKey {
                    reason: format!(
                        "Couldn't parse secret key from '{}': {}",
                        sign_key_path.display(),
                        e
                    ),
                })?;
        settings.secret_keys.push(key);
    }
    let virtual_store_dir = std::env::var_os("NIX_STORE_DIR")
        .map(|s| s.into_encoded_bytes())
        .unwrap_or_else(|| {
            settings
                .virtual_nix_store
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        });
    let real_store_path = settings
        .real_nix_store
        .clone()
        .unwrap_or_else(|| PathBuf::from(OsStr::from_bytes(&virtual_store_dir)));
    let db_path = settings
        .nix_db_path
        .clone()
        .or_else(|| derive_db_path(&real_store_path))
        .ok_or_else(|| ConfigError::Invalid {
            reason: format!(
                "could not derive nix_db_path from store dir {}; set nix_db_path explicitly",
                real_store_path.display()
            ),
        })?;
    if !db_path.exists() {
        return Err(ConfigError::Invalid {
            reason: format!(
                "nix database {} not found; set nix_db_path to the store's db.sqlite",
                db_path.display()
            ),
        }
        .into());
    }
    settings.store = Store::new(
        virtual_store_dir,
        settings
            .real_nix_store
            .clone()
            .map(|p| p.as_os_str().as_encoded_bytes().to_vec()),
        db_path,
    );
    Ok(settings)
}
