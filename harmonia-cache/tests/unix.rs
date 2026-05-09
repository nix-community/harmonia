use std::process::Command;

mod common;

use common::{CanonicalTempDir, LocalStore, start_harmonia_cache};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::test]
async fn test_unix_socket() -> Result<()> {
    let temp_dir = CanonicalTempDir::new()?;
    let store = LocalStore::init(temp_dir.path())?;

    let unix_socket_path = temp_dir.path().join("harmonia-socket");

    let cache_config = format!(
        r#"
bind = "unix:{}"
nix_db_path = "{}"
priority = 30
"#,
        unix_socket_path.display(),
        store.db_path().display(),
    );

    // Use a fake port for the helper function (it won't be used for Unix sockets)
    let _cache_guard = start_harmonia_cache(&cache_config, 0).await?;

    // Test Unix socket endpoint with curl
    println!("Testing Unix socket endpoint...");

    // Wait for the socket to be ready by trying curl in a loop
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(30);
    let cache_info;

    loop {
        if start.elapsed() > timeout {
            return Err("Timeout waiting for Unix socket to be ready".into());
        }

        let output = Command::new("curl")
            .args([
                "--unix-socket",
                unix_socket_path.to_str().unwrap(),
                "--fail",
                "--max-time",
                "2",
                "--silent",
                "http://localhost/nix-cache-info",
            ])
            .output()?;

        if output.status.success() {
            cache_info = String::from_utf8(output.stdout)?;
            println!("Unix socket is ready after {:?}", start.elapsed());
            break;
        }

        // Wait a bit before retrying
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    println!("Cache info response: {cache_info}");
    assert!(
        cache_info.contains("StoreDir:"),
        "Cache info should contain StoreDir"
    );
    assert!(
        cache_info.contains("Priority:"),
        "Cache info should contain Priority"
    );

    println!("Unix socket test completed successfully!");

    Ok(())
}
