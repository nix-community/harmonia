mod common;

use common::{Result, TestCacheBuilder};

#[tokio::test]
async fn test_build_trace_endpoint() -> Result<()> {
    let cache = TestCacheBuilder::new().build().await?;

    let status = cache.curl_status("/build-trace-v2/not-a-store-path/out.doi")?;
    assert_eq!(status, 400, "Expected 400 for invalid drv path");

    let status = cache
        .curl_status("/build-trace-v2/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv/out%7Bput.doi")?;
    assert_eq!(status, 400, "Expected 400 for invalid output name");

    let status =
        cache.curl_status("/build-trace-v2/g1w7hy3qg1w7hy3qg1w7hy3qg1w7hy3q-bar.drv/out.doi")?;
    assert_eq!(status, 404, "Expected 404 for non-existent realisation");

    Ok(())
}
