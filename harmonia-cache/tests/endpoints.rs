mod daemon;

use daemon::{Result, TestCache};

#[tokio::test]
async fn test_health_endpoint() -> Result<()> {
    let cache = TestCache::start().await?;

    let body = cache.curl("/health")?;
    assert_eq!(body, "OK\n");

    Ok(())
}

#[tokio::test]
async fn test_root_endpoint() -> Result<()> {
    let cache = TestCache::builder().priority(40).build().await?;

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
        response.contains("Priority: 40"),
        "Root endpoint should show priority"
    );

    Ok(())
}

#[tokio::test]
async fn test_root_endpoint_with_signing_keys() -> Result<()> {
    let cache = TestCache::builder()
        .signing_key(include_str!("../../tests/cache.sk"))
        .build()
        .await?;

    let body = cache.curl("/")?;

    assert!(
        body.contains("cache.example.com-1:"),
        "Root endpoint should show public key derived from signing key"
    );

    Ok(())
}
