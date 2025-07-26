use crate::handler::LocalStoreHandler;
use crate::sqlite::StoreDb;
use harmonia_store_remote::protocol::StorePath;
use harmonia_store_remote::server::RequestHandler;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn test_sqlite_with_nix_initialized_store() {
    // Create temporary directories
    let temp_dir = TempDir::new().unwrap();
    let store_dir = temp_dir.path().join("store");
    let state_dir = temp_dir.path().join("var/nix");

    // Create directory structure
    std::fs::create_dir_all(&store_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    // Initialize nix store - this creates the SQLite database
    let output = Command::new("nix-store")
        .arg("--init")
        .arg("--store")
        .arg(format!(
            "local?store={}&state={}",
            store_dir.display(),
            state_dir.display()
        ))
        .output()
        .expect("Failed to run nix-store --init");

    assert!(
        output.status.success(),
        "nix-store --init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify database was created
    let db_path = state_dir.join("db/db.sqlite");
    assert!(
        db_path.exists(),
        "Database should exist after nix-store --init"
    );

    // Now test our SQLite module can open and query the database
    let db = StoreDb::open(&db_path).expect("Failed to open database");

    // Copy something small to the store
    let hello_drv = Command::new("nix")
        .arg("eval")
        .arg("--raw")
        .arg("nixpkgs#hello.drvPath")
        .output()
        .expect("Failed to get hello derivation path");

    if hello_drv.status.success() {
        let drv_path = String::from_utf8_lossy(&hello_drv.stdout);

        // Build the store URL with explicit store path
        let store_url = format!(
            "local?store={}&state={}",
            store_dir.display(),
            state_dir.display()
        );

        // Copy the derivation to our test store
        let output = Command::new("nix")
            .arg("copy")
            .arg("--to")
            .arg(&store_url)
            .arg(drv_path.trim())
            .output()
            .expect("Failed to run nix copy");

        if output.status.success() {
            // List what's in the store
            let list_output = Command::new("nix")
                .arg("path-info")
                .arg("--store")
                .arg(&store_url)
                .arg("--all")
                .output()
                .expect("Failed to list store paths");

            println!(
                "Store contents:\n{}",
                String::from_utf8_lossy(&list_output.stdout)
            );

            // Get the first path from the output
            if let Some(first_line) = String::from_utf8_lossy(&list_output.stdout).lines().next() {
                if let Some(path) = first_line.split_whitespace().next() {
                    // The path should now be in our test store directory
                    let store_path = std::path::Path::new(path);

                    // Test is_valid_path
                    let is_valid = db.is_valid_path(store_path).unwrap();
                    assert!(is_valid, "Path {path} should be valid");

                    // Test query_path_info
                    let info = db.query_path_info(store_path).unwrap();
                    assert!(info.is_some(), "Should get path info for {path}");

                    // Extract hash part (first 32 chars of the base name)
                    if let Some(base_name) = store_path.file_name().and_then(|n| n.to_str()) {
                        if base_name.len() >= 32 {
                            let hash_part = &base_name[..32];

                            // Test query_path_from_hash_part
                            let found_path =
                                db.query_path_from_hash_part(&store_dir, hash_part).unwrap();
                            assert!(
                                found_path.is_some(),
                                "Should find path by hash part {hash_part}"
                            );
                        }
                    }
                }
            }
        }
    }

    // Even if we couldn't copy anything, at least verify the empty database works
    let is_valid = db
        .is_valid_path(&store_dir.join("non-existent-path"))
        .unwrap();
    assert!(!is_valid, "Non-existent path should not be valid");
}

#[tokio::test]
async fn test_handler_with_nix_store() {
    // Create temporary directories
    let temp_dir = TempDir::new().unwrap();
    let store_dir = temp_dir.path().join("store");
    let state_dir = temp_dir.path().join("var/nix");

    std::fs::create_dir_all(&store_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();

    // Initialize nix store
    let output = Command::new("nix-store")
        .arg("--init")
        .arg("--store")
        .arg(format!(
            "local?store={}&state={}",
            store_dir.display(),
            state_dir.display()
        ))
        .output()
        .expect("Failed to run nix-store --init");

    assert!(output.status.success(), "nix-store --init failed");

    let db_path = state_dir.join("db/db.sqlite");

    // Create handler
    let handler = LocalStoreHandler::new(store_dir.clone(), db_path)
        .await
        .expect("Failed to create handler");

    // Test with a non-existent path
    let fake_path = StorePath::from(format!(
        "{}/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-test",
        store_dir.display()
    ));
    let is_valid = handler.handle_is_valid_path(&fake_path).await.unwrap();
    assert!(!is_valid, "Non-existent path should not be valid");

    // Test query_path_info on non-existent path
    let info = handler.handle_query_path_info(&fake_path).await.unwrap();
    assert!(info.is_none(), "Should return None for non-existent path");

    // Test query_path_from_hash_part with non-existent hash
    let result = handler
        .handle_query_path_from_hash_part(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        .await
        .unwrap();
    assert!(
        result.is_none(),
        "Should return None for non-existent hash part"
    );
}
