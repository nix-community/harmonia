use std::io::Write;
use std::process::Command;

use tempfile::NamedTempFile;

mod daemon;

use daemon::{CanonicalTempDir, pick_unused_port, start_harmonia_cache};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct TestCache {
    port: u16,
    _temp_dir: CanonicalTempDir,
    _guard: Box<dyn Send>,
    _key_file: Option<NamedTempFile>,
}

impl TestCache {
    async fn start() -> Result<Self> {
        Self::start_with_options(None, None).await
    }

    async fn start_with_priority(priority: u32) -> Result<Self> {
        Self::start_with_options(Some(priority), None).await
    }

    async fn start_with_signing_key(key: &str) -> Result<Self> {
        Self::start_with_options(None, Some(key)).await
    }

    async fn start_with_options(priority: Option<u32>, signing_key: Option<&str>) -> Result<Self> {
        let temp_dir = CanonicalTempDir::new()?;
        let port = pick_unused_port().ok_or("No available ports")?;
        let store_dir = temp_dir.path().join("store");

        let key_file = if let Some(key) = signing_key {
            let mut file = NamedTempFile::new()?;
            write!(file, "{}", key.trim())?;
            file.flush()?;
            Some(file)
        } else {
            None
        };

        let mut config = format!(
            r#"
bind = "127.0.0.1:{port}"
virtual_nix_store = "{store}"
real_nix_store = "{store}"
"#,
            store = store_dir.display(),
        );

        if let Some(p) = priority {
            config.push_str(&format!("priority = {p}\n"));
        }

        if let Some(ref kf) = key_file {
            config.push_str(&format!("sign_key_paths = [\"{}\"]\n", kf.path().display()));
        }

        let guard = start_harmonia_cache(&config, port).await?;

        Ok(Self {
            port,
            _temp_dir: temp_dir,
            _guard: guard,
            _key_file: key_file,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn curl(&self, path: &str) -> Result<String> {
        let output = Command::new("curl")
            .args(["--fail", "--max-time", "2", "--silent", &self.url(path)])
            .output()?;

        assert!(
            output.status.success(),
            "Request to {} failed: {}",
            path,
            String::from_utf8_lossy(&output.stderr)
        );

        Ok(String::from_utf8(output.stdout)?)
    }

    fn curl_with_headers(&self, path: &str) -> Result<String> {
        let output = Command::new("curl")
            .args([
                "--fail",
                "--max-time",
                "2",
                "--silent",
                "--include",
                &self.url(path),
            ])
            .output()?;

        assert!(
            output.status.success(),
            "Request to {} failed: {}",
            path,
            String::from_utf8_lossy(&output.stderr)
        );

        Ok(String::from_utf8(output.stdout)?)
    }
}

#[tokio::test]
async fn test_health_endpoint() -> Result<()> {
    let cache = TestCache::start().await?;

    let body = cache.curl("/health")?;
    assert_eq!(body, "OK\n");

    Ok(())
}

#[tokio::test]
async fn test_root_endpoint() -> Result<()> {
    let cache = TestCache::start_with_priority(40).await?;

    let response = cache.curl_with_headers("/")?;

    assert!(
        response.contains("text/html"),
        "Root endpoint should return text/html"
    );
    assert!(
        response.contains("harmonia"),
        "Root endpoint should mention harmonia"
    );
    assert!(
        response.contains("priority=40"),
        "Root endpoint should show priority in nix-cache-info format"
    );

    Ok(())
}

#[tokio::test]
async fn test_root_endpoint_with_signing_keys() -> Result<()> {
    let signing_key = include_str!("../../tests/cache.sk");
    let cache = TestCache::start_with_signing_key(signing_key).await?;

    let body = cache.curl("/")?;

    assert!(
        body.contains("cache.example.com-1:"),
        "Root endpoint should show public key derived from signing key"
    );

    Ok(())
}
