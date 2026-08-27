mod common;

use common::{Result, TestCache};

#[tokio::test]
async fn test_prometheus_metrics() -> Result<()> {
    let cache = TestCache::builder().priority(30).build().await?;

    // Make request to a registered route
    cache.curl("/nix-cache-info")?;

    // Get metrics
    let metrics = cache.curl("/metrics")?;

    assert!(
        metrics.contains(r#"path="/nix-cache-info""#),
        "Metrics should include /nix-cache-info path"
    );

    // Arbitrary client-supplied methods must not become label values.
    std::process::Command::new("curl")
        .args(["-s", "-X", "FOO123", &cache.url("/nix-cache-info")])
        .output()?;
    let metrics = cache.curl("/metrics")?;
    assert!(!metrics.contains("FOO123"), "unbounded method label");

    Ok(())
}
