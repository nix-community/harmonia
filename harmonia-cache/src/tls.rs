use crate::error::{Result, ServerError as ServerErrorType};
use rustls::ServerConfig;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::ffi::OsString;
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
    if let Some(mode) = insecure_permissions(path, std::env::var_os("CREDENTIALS_DIRECTORY")) {
        tracing::warn!(
            "{} has insecure permissions {:#o}; recommend 0600",
            path.display(),
            mode
        );
    }
}

/// Returns the file mode if it is group/other-readable, `None` otherwise.
///
/// Files under `$CREDENTIALS_DIRECTORY` are exempt: systemd provisions
/// credentials with mode 0440 but restricts access via ACLs, so the mode bits
/// alone are not meaningful (systemd/systemd#29435).
fn insecure_permissions(path: &Path, credentials_dir: Option<OsString>) -> Option<u32> {
    if let Some(dir) = credentials_dir
        && let (Ok(p), Ok(d)) = (path.canonicalize(), Path::new(&dir).canonicalize())
        && p.starts_with(&d)
    {
        return None;
    }

    let meta = std::fs::metadata(path).ok()?;
    let mode = meta.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::Permissions;

    fn key_file(dir: &Path, mode: u32) -> std::path::PathBuf {
        let path = dir.join("key.pem");
        std::fs::write(&path, "secret").unwrap();
        std::fs::set_permissions(&path, Permissions::from_mode(mode)).unwrap();
        path
    }

    #[test]
    fn flags_group_or_other_readable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            insecure_permissions(&key_file(dir.path(), 0o644), None),
            Some(0o644)
        );
        assert_eq!(
            insecure_permissions(&key_file(dir.path(), 0o600), None),
            None
        );
    }

    #[test]
    fn skips_files_under_credentials_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = key_file(dir.path(), 0o440);
        let creds = Some(dir.path().as_os_str().to_owned());
        assert_eq!(insecure_permissions(&path, creds), None);
    }

    #[test]
    fn still_warns_outside_credentials_directory() {
        let dir = tempfile::tempdir().unwrap();
        let creds_dir = tempfile::tempdir().unwrap();
        let path = key_file(dir.path(), 0o644);
        let creds = Some(creds_dir.path().as_os_str().to_owned());
        assert_eq!(insecure_permissions(&path, creds), Some(0o644));
    }
}
