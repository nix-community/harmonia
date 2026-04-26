use crate::error::{IoErrorContext, Result, ServerError as ServerErrorType};
use rustls::ServerConfig;
use rustls_pemfile::{certs, private_key};
use std::fs::File;
use std::io::BufReader;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub fn load_tls_config(cert_path: &Path, key_path: &Path) -> Result<ServerConfig> {
    warn_insecure_permissions(key_path);
    // Load certificate chain
    let cert_file = File::open(cert_path).io_context("Failed to open certificate file")?;
    let mut cert_reader = BufReader::new(cert_file);
    let mut cert_chain = Vec::new();
    for cert in certs(&mut cert_reader) {
        cert_chain.push(cert.map_err(|e| ServerErrorType::TlsSetup {
            reason: format!("Failed to parse certificate: {e}"),
        })?);
    }

    // Accepts PKCS8/PKCS1/SEC1 and surfaces the real parse error.
    let key_file = File::open(key_path).io_context("Failed to open private key file")?;
    let mut key_reader = BufReader::new(key_file);
    let key = private_key(&mut key_reader)
        .map_err(|e| ServerErrorType::TlsSetup {
            reason: format!("Failed to parse private key {}: {e}", key_path.display()),
        })?
        .ok_or_else(|| ServerErrorType::TlsSetup {
            reason: format!(
                "No PKCS8/PKCS1/SEC1 private key found in {}",
                key_path.display()
            ),
        })?;

    // Create rustls config
    ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .map_err(|e| ServerErrorType::TlsSetup {
            reason: format!("Failed to create TLS config: {e}"),
        })
        .map_err(Into::into)
}

/// Warn (not fail) when a secret file is group/other-readable.
pub(crate) fn warn_insecure_permissions(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            tracing::warn!(
                "{} has insecure permissions {:#o}; recommend 0600",
                path.display(),
                mode
            );
        }
    }
}
