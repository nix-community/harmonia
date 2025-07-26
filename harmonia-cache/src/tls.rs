use crate::error::{IoErrorContext, Result, ServerError as ServerErrorType};
use rustls::ServerConfig;
use rustls_pemfile::{certs, pkcs8_private_keys};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

pub fn load_tls_config(cert_path: &Path, key_path: &Path) -> Result<ServerConfig> {
    // Load certificate chain
    let cert_file = File::open(cert_path).io_context("Failed to open certificate file")?;
    let mut cert_reader = BufReader::new(cert_file);
    let mut cert_chain = Vec::new();
    for cert in certs(&mut cert_reader) {
        cert_chain.push(cert.map_err(|e| ServerErrorType::TlsSetup {
            reason: format!("Failed to parse certificate: {e}"),
        })?);
    }

    // Load private key
    let key_file = File::open(key_path).io_context("Failed to open private key file")?;
    let mut key_reader = BufReader::new(key_file);
    let mut keys = Vec::new();
    for key in pkcs8_private_keys(&mut key_reader) {
        keys.push(key.map_err(|e| ServerErrorType::TlsSetup {
            reason: format!("Failed to parse private key: {e}"),
        })?);
    }

    if keys.is_empty() {
        return Err(ServerErrorType::TlsSetup {
            reason: "No private key found in file".to_string(),
        }
        .into());
    }

    let key = keys.into_iter().next().unwrap().into();

    // Create rustls config
    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|e| ServerErrorType::TlsSetup {
            reason: format!("Failed to create TLS config: {e}"),
        })
        .map_err(Into::into)
}
