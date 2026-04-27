//! Malformed store-path hashes must yield 4xx, never 5xx.

mod common;

use common::{Result, TestCache};

#[tokio::test]
async fn malformed_hash_is_4xx_not_5xx() -> Result<()> {
    let cache = TestCache::start().await?;

    let cases = [
        // These two returned 500 before the fix.
        "/%2e%2e%2f%2e%2e%2fetc%2fpasswd.ls",
        "/log/..%2f..%2f..%2f..%2f..%2f..%2f..%2fetc%2fpasswd",
        // Pin that the other hash entry points keep validating.
        "/%2e%2e%2fetc%2fpasswd.narinfo",
        "/nar/0000000000000000000000000000000000000000000000000000.nar?hash=../../etc/passwd",
    ];

    for path in cases {
        let status = cache.curl_status(path)?;
        assert!(
            (400..500).contains(&status),
            "{path} must be rejected with 4xx, got {status}",
        );
    }

    Ok(())
}
