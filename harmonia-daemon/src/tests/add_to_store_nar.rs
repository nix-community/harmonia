// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Tests for the `add_to_store_nar` handler method.

use std::collections::BTreeSet;

use harmonia_nar::archive::{test_data, write_nar};
use harmonia_protocol::NarHash;
use harmonia_protocol::daemon::{DaemonStore, HandshakeDaemonStore};
use harmonia_protocol::valid_path_info::{UnkeyedValidPathInfo, ValidPathInfo};
use harmonia_store_core::store_path::{StoreDir, StorePath};
use tokio::io::AsyncReadExt as _;

use harmonia_protocol::daemon::AddToStoreItem;
use harmonia_store_core::signature::{SecretKey, fingerprint_path};

use super::test_store::TestStore;

/// Helper: create a ValidPathInfo for a test path with the given NAR bytes.
fn make_path_info(store_dir: &StoreDir, name: &str, nar_bytes: &[u8]) -> ValidPathInfo {
    let nar_hash = NarHash::digest(nar_bytes);
    let path = StorePath::from_base_path(name).unwrap();
    ValidPathInfo {
        path,
        info: UnkeyedValidPathInfo {
            deriver: None,
            nar_hash,
            references: BTreeSet::new(),
            registration_time: None,
            nar_size: nar_bytes.len() as u64,
            ultimate: true,
            signatures: BTreeSet::new(),
            ca: None,
            store_dir: store_dir.clone(),
        },
    }
}

/// Call `add_to_store_nar` with a valid NAR + ValidPathInfo →
/// path exists on disk, registered in DB with correct hash, references, and nar_size.
#[tokio::test]
async fn test_add_to_store_nar_valid_path() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    // Create NAR bytes for a simple text file
    let events = test_data::text_file();
    let nar_bytes = write_nar(&events);

    let info = make_path_info(
        &ts.store_dir,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello",
        &nar_bytes,
    );

    // Call add_to_store_nar
    let cursor = std::io::Cursor::new(nar_bytes.to_vec());
    let result = store.add_to_store_nar(&info, cursor, false, true).await;
    result.unwrap();

    // Verify path exists on disk
    let store_path = ts
        .store_path()
        .join("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-hello");
    assert!(store_path.exists(), "Store path should exist on disk");

    // Read the file content to verify NAR was unpacked correctly
    let content = std::fs::read_to_string(&store_path).unwrap();
    assert_eq!(content, "Hello world!");

    // Verify path is registered in the database
    let is_valid = store.is_valid_path(&info.path).await.unwrap();
    assert!(is_valid, "Path should be registered as valid in DB");

    // Verify path info matches
    let db_info = store.query_path_info(&info.path).await.unwrap();
    let db_info = db_info.expect("Should have path info in DB");
    assert_eq!(db_info.nar_hash, info.info.nar_hash);
    assert_eq!(db_info.nar_size, info.info.nar_size);
    assert_eq!(db_info.references, info.info.references);
}

/// NAR whose hash doesn't match declared narHash →
/// error, nothing on disk, nothing in DB.
#[tokio::test]
async fn test_add_to_store_nar_hash_mismatch() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let events = test_data::text_file();
    let nar_bytes = write_nar(&events);

    // Create path info with a WRONG hash (all zeros)
    let wrong_hash = NarHash::new(&[0u8; 32]);
    let path = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bad-hash").unwrap();
    let info = ValidPathInfo {
        path: path.clone(),
        info: UnkeyedValidPathInfo {
            deriver: None,
            nar_hash: wrong_hash,
            references: BTreeSet::new(),
            registration_time: None,
            nar_size: nar_bytes.len() as u64,
            ultimate: true,
            signatures: BTreeSet::new(),
            ca: None,
            store_dir: ts.store_dir.clone(),
        },
    };

    let cursor = std::io::Cursor::new(nar_bytes.to_vec());
    let result = store.add_to_store_nar(&info, cursor, false, true).await;
    assert!(result.is_err(), "Should fail with hash mismatch");

    // Verify nothing on disk
    let store_path = ts
        .store_path()
        .join("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bad-hash");
    assert!(
        !store_path.exists(),
        "No path should exist on disk after hash mismatch"
    );

    // Verify nothing in DB
    let is_valid = store.is_valid_path(&path).await.unwrap();
    assert!(!is_valid, "Path should NOT be in DB after hash mismatch");
}

/// `dont_check_sigs = false` with no valid signature → error;
/// with valid signature → succeeds.
#[tokio::test]
async fn test_add_to_store_nar_signature_verification() {
    // Generate a keypair
    let secret_key =
        SecretKey::generate("test-key".to_string(), &ring::rand::SystemRandom::new()).unwrap();
    let public_key = secret_key.to_public_key();

    let ts = TestStore::with_trusted_keys(vec![public_key]);
    let mut store = ts.handler.clone().handshake().await.unwrap();

    let events = test_data::text_file();
    let nar_bytes = write_nar(&events);
    let nar_hash = NarHash::digest(&nar_bytes);

    let path_name = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-sig-test";
    let path = StorePath::from_base_path(path_name).unwrap();
    let references = BTreeSet::new();

    // --- Case 1: no signature, dont_check_sigs=false → should fail ---
    let info_no_sig = ValidPathInfo {
        path: path.clone(),
        info: UnkeyedValidPathInfo {
            deriver: None,
            nar_hash,
            references: references.clone(),
            registration_time: None,
            nar_size: nar_bytes.len() as u64,
            ultimate: true,
            signatures: BTreeSet::new(),
            ca: None,
            store_dir: ts.store_dir.clone(),
        },
    };

    let cursor = std::io::Cursor::new(nar_bytes.to_vec());
    let result = store
        .add_to_store_nar(&info_no_sig, cursor, false, false)
        .await;
    assert!(result.is_err(), "Should fail without valid signature");

    // --- Case 2: valid signature, dont_check_sigs=false → should succeed ---
    let nar_hash_str = format!(
        "{}",
        harmonia_utils_hash::fmt::CommonHash::as_base32(&nar_hash)
    );
    let fp = fingerprint_path(
        &ts.store_dir,
        &path,
        nar_hash_str.as_bytes(),
        nar_bytes.len() as u64,
        &references,
    )
    .unwrap();
    let sig = secret_key.sign(&fp);

    let mut sigs = BTreeSet::new();
    sigs.insert(sig);

    let info_with_sig = ValidPathInfo {
        path: path.clone(),
        info: UnkeyedValidPathInfo {
            deriver: None,
            nar_hash,
            references: references.clone(),
            registration_time: None,
            nar_size: nar_bytes.len() as u64,
            ultimate: true,
            signatures: sigs,
            ca: None,
            store_dir: ts.store_dir.clone(),
        },
    };

    let cursor = std::io::Cursor::new(nar_bytes.to_vec());
    let result = store
        .add_to_store_nar(&info_with_sig, cursor, false, false)
        .await;
    assert!(
        result.is_ok(),
        "Should succeed with valid signature: {:?}",
        result.err()
    );

    // Verify it's in the store
    let is_valid = store.is_valid_path(&path).await.unwrap();
    assert!(is_valid, "Path should be registered after valid signature");
}

/// Stream of 3 valid paths → all 3 on disk and in DB.
#[tokio::test]
async fn test_add_multiple_to_store_three_paths() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    // Create 3 different NAR payloads
    let test_cases = vec![
        (
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-one",
            test_data::text_file(),
        ),
        (
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-two",
            test_data::exec_file(),
        ),
        (
            "cccccccccccccccccccccccccccccccc-three",
            test_data::empty_file(),
        ),
    ];

    let mut items = Vec::new();
    let mut paths = Vec::new();
    for (name, events) in &test_cases {
        let nar_bytes = write_nar(events);
        let info = make_path_info(&ts.store_dir, name, &nar_bytes);
        paths.push(info.path.clone());
        items.push(Ok(AddToStoreItem {
            info,
            reader: std::io::Cursor::new(nar_bytes.to_vec()),
        }));
    }

    let stream = futures::stream::iter(items);
    store
        .add_multiple_to_store(false, true, stream)
        .await
        .unwrap();

    // Verify all 3 exist on disk and in DB
    for (i, path) in paths.iter().enumerate() {
        let is_valid = store.is_valid_path(path).await.unwrap();
        assert!(is_valid, "Path {} should be valid in DB", i);

        let disk_path = ts.store_path().join(path.to_string());
        assert!(disk_path.exists(), "Path {} should exist on disk", i);
    }
}

/// Second path in batch has bad hash → first path kept, second rejected with error.
#[tokio::test]
async fn test_add_multiple_to_store_partial_failure() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    // First path: valid
    let events1 = test_data::text_file();
    let nar_bytes1 = write_nar(&events1);
    let info1 = make_path_info(
        &ts.store_dir,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-good",
        &nar_bytes1,
    );
    let path1 = info1.path.clone();

    // Second path: bad hash
    let events2 = test_data::exec_file();
    let nar_bytes2 = write_nar(&events2);
    let wrong_hash = NarHash::new(&[0u8; 32]);
    let path2 = StorePath::from_base_path("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-bad").unwrap();
    let info2 = ValidPathInfo {
        path: path2.clone(),
        info: UnkeyedValidPathInfo {
            deriver: None,
            nar_hash: wrong_hash,
            references: BTreeSet::new(),
            registration_time: None,
            nar_size: nar_bytes2.len() as u64,
            ultimate: true,
            signatures: BTreeSet::new(),
            ca: None,
            store_dir: ts.store_dir.clone(),
        },
    };

    let items = vec![
        Ok(AddToStoreItem {
            info: info1,
            reader: std::io::Cursor::new(nar_bytes1.to_vec()),
        }),
        Ok(AddToStoreItem {
            info: info2,
            reader: std::io::Cursor::new(nar_bytes2.to_vec()),
        }),
    ];

    let stream = futures::stream::iter(items);
    let result = store.add_multiple_to_store(false, true, stream).await;
    assert!(
        result.is_err(),
        "Batch should fail due to bad hash on second path"
    );

    // First path should still be in DB (was registered before failure)
    let is_valid1 = store.is_valid_path(&path1).await.unwrap();
    assert!(is_valid1, "First path should be kept in DB");

    // Second path should NOT be in DB
    let is_valid2 = store.is_valid_path(&path2).await.unwrap();
    assert!(!is_valid2, "Second path should NOT be in DB");
}

/// Add a path via `add_to_store_nar`, then `nar_from_path` returns NAR
/// whose SHA-256 matches the registered `narHash`.
#[tokio::test]
async fn test_nar_from_path_roundtrip() {
    use crate::handler::LocalStoreHandler;

    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    // Add a path
    let events = test_data::text_file();
    let nar_bytes = write_nar(&events);
    let info = make_path_info(
        &ts.store_dir,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-roundtrip",
        &nar_bytes,
    );
    let expected_hash = info.info.nar_hash;

    let cursor = std::io::Cursor::new(nar_bytes.to_vec());
    store
        .add_to_store_nar(&info, cursor, false, true)
        .await
        .unwrap();

    // Read it back via nar_from_path_impl
    let mut reader = LocalStoreHandler::nar_from_path_impl(&ts.store_dir, &info.path)
        .await
        .unwrap();
    let mut output_bytes = Vec::new();
    reader.read_to_end(&mut output_bytes).await.unwrap();

    // Verify hash matches
    let actual_hash = NarHash::digest(&output_bytes);
    assert_eq!(
        actual_hash, expected_hash,
        "NAR hash from nar_from_path should match the registered narHash"
    );
}
