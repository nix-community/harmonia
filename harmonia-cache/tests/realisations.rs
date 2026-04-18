mod daemon;

use daemon::{CanonicalTempDir, Daemon, DaemonConfig, NixDaemon, Result, TestCacheBuilder};
use std::process::Command;

fn curl_status(url: &str) -> Result<String> {
    let output = Command::new("curl")
        .args([
            "--silent",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
        ])
        .arg(url)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "curl {url} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[tokio::test]
async fn test_build_trace_endpoint() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;
    let store_dir = temp_dir.path().join("store");
    let state_dir = temp_dir.path().join("var");

    let daemon_config = DaemonConfig {
        socket_path: temp_dir.path().join("nix-daemon.sock"),
        store_dir: store_dir.clone(),
        state_dir: state_dir.clone(),
    };

    let daemon = NixDaemon::start(daemon_config).await?;
    let cache = TestCacheBuilder::new().daemon(daemon).build().await?;

    let status = curl_status(&cache.url("/build-trace-v2/not-a-store-path/out.doi"))?;
    assert_eq!(status, "400", "Expected 400 for invalid drv path");

    let status = curl_status(
        &cache.url("/build-trace-v2/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv/out%7Bput.doi"),
    )?;
    assert_eq!(status, "400", "Expected 400 for invalid output name");

    let status = curl_status(
        &cache.url("/build-trace-v2/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv/out.doi"),
    )?;
    assert_eq!(status, "404", "Expected 404 for non-existent realisation");

    Ok(())
}
