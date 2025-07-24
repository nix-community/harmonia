use crate::signing::parse_secret_key;
use crate::store::Store;
use anyhow::{bail, Context, Result};
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

#[derive(Debug)]
pub(crate) struct SigningKey {
    pub(crate) name: String,
    pub(crate) key: Vec<u8>,
}

// TODO(conni2461): users to restrict access
#[derive(Deserialize, Debug)]
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

    #[serde(skip, default)]
    pub(crate) secret_keys: Vec<SigningKey>,
    #[serde(skip)]
    pub(crate) store: Store,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: default_bind(),
            workers: default_workers(),
            max_connection_rate: default_connection_rate(),
            priority: default_priority(),
            virtual_nix_store: default_virtual_store(),
            real_nix_store: None,
            sign_key_path: None,
            sign_key_paths: Vec::new(),
            tls_cert_path: None,
            tls_key_path: None,
            secret_keys: Vec::new(),
            store: Store::new(default_virtual_store(), None),
        }
    }
}

impl Config {
    pub(crate) fn load(settings_file: &Path) -> Result<Config> {
        toml::from_str(
            &read_to_string(settings_file).with_context(|| {
                format!("Couldn't read config file '{}'", settings_file.display())
            })?,
        )
        .with_context(|| format!("Couldn't parse config file '{}'", settings_file.display()))
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
        bail!("workers must be greater than 0");
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
            .push(parse_secret_key(sign_key_path).with_context(|| {
                format!(
                    "Couldn't parse secret key from '{}'",
                    sign_key_path.display()
                )
            })?);
    }
    let store_dir = std::env::var_os("NIX_STORE_DIR")
        .map(|s| s.into_encoded_bytes())
        .unwrap_or_else(|| settings.virtual_nix_store.clone());
    settings.store = Store::new(
        store_dir,
        settings.real_nix_store.clone().map(|s| s.into_bytes()),
    );
    Ok(settings)
}
