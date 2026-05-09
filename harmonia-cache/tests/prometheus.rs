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

    Ok(())
}
