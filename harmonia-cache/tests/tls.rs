mod common;

use common::{Result, TestCache};

const TLS_CERT: &str = include_str!("../../tests/tls-cert.pem");
const TLS_KEY: &str = include_str!("../../tests/tls-key.pem");

#[tokio::test]
async fn test_tls() -> Result<()> {
    let cache = TestCache::builder()
        .priority(30)
        .tls(TLS_CERT, TLS_KEY)
        .build()
        .await?;

    // Test HTTPS endpoints
    let version = cache.curl("/version")?;
    assert!(
        !version.is_empty(),
        "Version endpoint should return content"
    );

    let cache_info = cache.curl("/nix-cache-info")?;
    assert!(
        cache_info.contains("StoreDir:"),
        "Invalid nix-cache-info response"
    );
    assert!(
        cache_info.contains("Priority: 30"),
        "Invalid priority in response"
    );

    Ok(())
}
