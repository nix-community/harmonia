use crate::error::{Result, ServerError as ServerErrorType};
use rustls::ServerConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub fn load_tls_config(cert_path: &Path, key_path: &Path) -> Result<ServerConfig> {
    warn_insecure_permissions(key_path);

    let cert_chain = CertificateDer::pem_file_iter(cert_path)
        .map_err(|e| ServerErrorType::TlsSetup {
            reason: format!(
                "Failed to open certificate file {}: {e}",
                cert_path.display()
            ),
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| ServerErrorType::TlsSetup {
            reason: format!("Failed to parse certificate: {e}"),
        })?;

    // `PrivateKeyDer::from_pem_file` accepts PKCS#8, PKCS#1 and SEC1.
    let key = PrivateKeyDer::from_pem_file(key_path).map_err(|e| ServerErrorType::TlsSetup {
        reason: format!(
            "Failed to read private key from {}: {e}",
            key_path.display()
        ),
    })?;

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
