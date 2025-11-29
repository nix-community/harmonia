mod daemon;

use daemon::{CanonicalTempDir, Daemon, DaemonConfig, HarmoniaDaemon, Result, TestCache};

#[tokio::test]
async fn test_prometheus_metrics() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;

    let daemon = HarmoniaDaemon::start(DaemonConfig {
        socket_path: temp_dir.path().join("harmonia-daemon.sock"),
        store_dir: temp_dir.path().join("store"),
        state_dir: temp_dir.path().join("var"),
    })
    .await?;

    let cache = TestCache::builder()
        .daemon(daemon)
        .priority(30)
        .build()
        .await?;

    // Make request to a registered route
    cache.curl("/nix-cache-info")?;

    // Get metrics
    let metrics = cache.curl("/metrics")?;

    assert!(
        metrics.contains(r#"path="/nix-cache-info""#),
        "Metrics should include /nix-cache-info path"
    );

    Ok(())
}
