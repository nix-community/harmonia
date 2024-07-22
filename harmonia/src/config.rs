use std::fs::read_to_string;

use crate::store::Store;
use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine};
use serde::Deserialize;

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

// TODO(conni2461): users to restrict access
#[derive(Deserialize, Debug)]
pub(crate) struct Config {
    #[serde(default = "default_bind")]
    pub(crate) bind: String,
    #[serde(default = "default_workers")]
    pub(crate) workers: usize,
    #[serde(default = "default_connection_rate")]
    pub(crate) max_connection_rate: usize,
    #[serde(default = "default_priority")]
    pub(crate) priority: usize,
    #[serde(default)]
    pub(crate) sign_key_path: Option<String>,
    #[serde(default)]
    pub(crate) sign_key_paths: Vec<String>,
    #[serde(default)]
    pub(crate) tls_cert_path: Option<String>,
    #[serde(default)]
    pub(crate) tls_key_path: Option<String>,

    #[serde(skip, default)]
    pub(crate) secret_keys: Vec<String>,
    #[serde(skip)]
    pub(crate) store: Store,
}

fn get_secret_key(sign_key_path: Option<&str>) -> Result<Option<String>> {
    if let Some(path) = sign_key_path {
        let sign_key = read_to_string(path)
            .with_context(|| format!("Couldn't read sign_key file '{path}'"))?;
        let (_sign_host, sign_key64) = sign_key
            .split_once(':')
            .with_context(|| format!("Sign key in '{path}' does not contain a ':'"))?;
        let sign_keyno64 = general_purpose::STANDARD
            .decode(sign_key64.trim())
            .with_context(|| format!("Couldn't base64::decode sign key from '{path}'"))?;
        if sign_keyno64.len() == 64 {
            return Ok(Some(sign_key.to_owned()));
        }
        log::error!("invalid signing key provided. signing disabled");
    }
    Ok(None)
}

pub(crate) fn load() -> Result<Config> {
    let settings_file = std::env::var("CONFIG_FILE").unwrap_or_else(|_| "settings.toml".to_owned());
    let mut settings: Config = toml::from_str(
        &read_to_string(&settings_file)
            .with_context(|| format!("Couldn't read config file '{settings_file}'"))?,
    )
    .with_context(|| format!("Couldn't parse config file '{settings_file}'"))?;
    if let Some(sign_key_path) = &settings.sign_key_path {
        log::warn!(
            "The sign_key_path configuration option is deprecated. Use sign_key_paths instead."
        );
        settings.sign_key_paths.push(sign_key_path.to_string());
    }
    if let Ok(sign_key_path) = std::env::var("SIGN_KEY_PATH") {
        log::warn!(
            "The SIGN_KEY_PATH environment variable is deprecated. Use SIGN_KEY_PATHS instead."
        );
        settings.sign_key_paths.push(sign_key_path.to_string());
    }
    if let Ok(sign_key_paths) = std::env::var("SIGN_KEY_PATHS") {
        for sign_key_path in sign_key_paths.split_whitespace() {
            settings.sign_key_paths.push(sign_key_path.to_string());
        }
    }
    for sign_key_path in &settings.sign_key_paths {
        if let Some(sk) = get_secret_key(Some(sign_key_path))? {
            settings.secret_keys.push(sk);
        }
    }
    settings.store = Store::new();
    Ok(settings)
}
