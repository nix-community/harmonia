// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

use crate::handler::LocalStoreHandler;
use harmonia_protocol::daemon::{DaemonStore, HandshakeDaemonStore};
use harmonia_store_core::store_path::{StoreDir, StorePath};
use harmonia_store_db::StoreDb;
use harmonia_utils_test::CanonicalTempDir;
use std::process::Command;

#[test]
fn test_sqlite_with_nix_initialized_store() {
    // Create temporary directories (canonicalized for macOS /var symlink)
    let temp_dir = CanonicalTempDir::new().unwrap();
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
    let db = StoreDb::open(&db_path, harmonia_store_db::OpenMode::ReadOnly)
        .expect("Failed to open database");

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
            if let Some(path) = String::from_utf8_lossy(&list_output.stdout)
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().next())
            {
                let sd =
                    StoreDir::new(store_dir.to_str().unwrap_or("/nix/store")).unwrap_or_default();
                if let Ok(sp) = sd.parse(path) {
                    // Test is_valid_path
                    let is_valid = db.is_valid_path(&sd, &sp).unwrap();
                    assert!(is_valid, "Path {path} should be valid");

                    // Test query_path_info
                    let info = db.query_path_info(&sd, &sp).unwrap();
                    assert!(info.is_some(), "Should get path info for {path}");

                    // Test query_path_from_hash_part
                    let found_path = db.query_path_from_hash_part(&sd, sp.hash()).unwrap();
                    assert!(
                        found_path.is_some(),
                        "Should find path by hash part {}",
                        sp.hash()
                    );
                }
            }
        }
    }

    // Even if we couldn't copy anything, at least verify the empty database works
    {
        let sd = StoreDir::new(store_dir.to_str().unwrap_or("/nix/store")).unwrap_or_default();
        let fake_path: StorePath = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-non-existent"
            .parse()
            .unwrap();
        let is_valid = db.is_valid_path(&sd, &fake_path).unwrap();
        assert!(!is_valid, "Non-existent path should not be valid");
    }
}

#[tokio::test]
async fn test_handler_with_nix_store() {
    // Create temporary directories (canonicalized for macOS /var symlink)
    let temp_dir = CanonicalTempDir::new().unwrap();
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
    let store_dir = StoreDir::new(store_dir).expect("Failed to create StoreDir");
    let handler = LocalStoreHandler::new(store_dir, db_path, false)
        .await
        .expect("Failed to create handler");

    // Complete handshake to get a DaemonStore
    let mut store = handler.handshake().await.expect("Handshake failed");

    // Test with a non-existent path
    let fake_path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-test").unwrap();
    let is_valid = store.is_valid_path(&fake_path).await.unwrap();
    assert!(!is_valid, "Non-existent path should not be valid");

    // Test query_path_info on non-existent path
    let info = store.query_path_info(&fake_path).await.unwrap();
    assert!(info.is_none(), "Should return None for non-existent path");

    // Test query_path_from_hash_part with non-existent hash
    // Create a fake hash with arbitrary bytes (20 bytes for store path hash)
    let fake_hash = harmonia_store_core::store_path::StorePathHash::copy_from_slice(&[0u8; 20]);
    let result = store.query_path_from_hash_part(&fake_hash).await.unwrap();
    assert!(
        result.is_none(),
        "Should return None for non-existent hash part"
    );
}

/// `query_realisation` against a hand-populated `BuildTraceV3` table.
#[tokio::test]
async fn test_handler_query_realisation() {
    let temp_dir = CanonicalTempDir::new().unwrap();
    let store_dir = StoreDir::new("/nix/store").unwrap();
    let db_path = temp_dir.path().join("db.sqlite");

    let drv_base = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-ca.drv";
    let sig = "cache.nixos.org-1:0CpHca+06TwFp9VkMyz5OaphT3E8mnS+1SWymYlvFaghKSYPCMQ66TS1XPAr1+y9rfQZPLaHrBjjnIRktE/nAA==";

    {
        let db = StoreDb::open(&db_path, harmonia_store_db::OpenMode::Create).unwrap();
        db.create_schema().unwrap();
        db.register_realisation(
            &store_dir,
            &harmonia_store_core::realisation::Realisation {
                key: harmonia_store_core::realisation::DrvOutput {
                    drv_path: drv_base.parse().unwrap(),
                    output_name: "out".parse().unwrap(),
                },
                value: harmonia_store_core::realisation::UnkeyedRealisation {
                    out_path: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-out".parse().unwrap(),
                    signatures: [sig.parse().unwrap()].into_iter().collect(),
                },
            },
        )
        .unwrap();
    }

    let handler = LocalStoreHandler::new(store_dir, db_path, false).await.unwrap();
    let mut store = handler.handshake().await.unwrap();

    let id = harmonia_store_core::realisation::DrvOutput {
        drv_path: StorePath::from_base_path(drv_base).unwrap(),
        output_name: "out".parse().unwrap(),
    };
    let r = store.query_realisation(&id).await.unwrap().unwrap();
    assert_eq!(
        r.out_path,
        StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-out").unwrap()
    );
    assert_eq!(r.signatures.len(), 1);
    assert_eq!(
        r.signatures.iter().next().unwrap().name(),
        "cache.nixos.org-1"
    );

    let miss = harmonia_store_core::realisation::DrvOutput {
        drv_path: StorePath::from_base_path(drv_base).unwrap(),
        output_name: "dev".parse().unwrap(),
    };
    assert!(store.query_realisation(&miss).await.unwrap().is_none());
}
