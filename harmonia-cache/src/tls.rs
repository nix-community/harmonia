use crate::error::{IoErrorContext, Result, ServerError as ServerErrorType};
use rustls::ServerConfig;
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use rustls_pki_types::PrivateKeyDer;
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

    // Load private key from PEM file - try PKCS8 first
    let key_file = File::open(key_path).io_context("Failed to open private key file")?;
    let mut key_reader = BufReader::new(key_file);
    let pkcs8_keys: Vec<_> = pkcs8_private_keys(&mut key_reader)
        .filter_map(|k| k.ok())
        .collect();

    let key = if !pkcs8_keys.is_empty() {
        PrivateKeyDer::Pkcs8(
            pkcs8_keys
                .into_iter()
                .next()
                .expect("failed to extract first PKCS8 private key from non-empty key collection"),
        )
    } else {
        // Try RSA format
        let key_file = File::open(key_path).io_context("Failed to reopen private key file")?;
        let mut key_reader = BufReader::new(key_file);
        let rsa_keys: Vec<_> = rsa_private_keys(&mut key_reader)
            .filter_map(|k| k.ok())
            .collect();

        if rsa_keys.is_empty() {
            return Err(ServerErrorType::TlsSetup {
                reason: "No valid private key found in PEM file (tried PKCS8 and RSA formats)"
                    .to_string(),
            }
            .into());
        }

        PrivateKeyDer::Pkcs1(
            rsa_keys
                .into_iter()
                .next()
                .expect("rsa keys vector is not empty but iterator returned None"),
        )
    };

    // Create rustls config
    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|e| ServerErrorType::TlsSetup {
            reason: format!("Failed to create TLS config: {e}"),
        })
        .map_err(Into::into)
}
