// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Smoke tests for harmonia-store-db.
//!
//! These tests verify the schema and basic operations work correctly
//! using an in-memory database.

use std::collections::BTreeSet;

use harmonia_store_db::{RegisterPathParams, StoreDb};

fn make_path(hash: &str, name: &str) -> String {
    format!("/nix/store/{hash}-{name}")
}

/// Verify schema creation and empty queries work.
#[test]
fn test_schema_creation() {
    let db = StoreDb::open_memory().unwrap();
    assert!(db.has_schema().unwrap());
    assert!(db.has_ca_schema().unwrap());
    assert_eq!(db.count_valid_paths().unwrap(), 0);
}

/// Verify path registration and query roundtrip.
#[test]
fn test_path_roundtrip() {
    let mut db = StoreDb::open_memory().unwrap();

    let params = RegisterPathParams {
        path: make_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "hello"),
        hash: "sha256:".to_string() + &"0".repeat(64),
        nar_size: Some(12345),
        ultimate: true,
        sigs: Some("cache.example.com:abc123".into()),
        ..Default::default()
    };

    let id = db.register_valid_path(&params).unwrap();
    assert!(id > 0);

    let info = db.query_path_info(&params.path).unwrap().unwrap();
    assert_eq!(info.path, params.path);
    assert_eq!(info.hash, params.hash);
    assert_eq!(info.nar_size, params.nar_size);
    assert!(info.ultimate);
    assert!(info.is_signed());
}

/// Verify reference graph operations.
#[test]
fn test_reference_graph() {
    let mut db = StoreDb::open_memory().unwrap();

    // Create a dependency chain: app -> lib -> glibc
    let glibc = RegisterPathParams {
        path: make_path("gggggggggggggggggggggggggggggggg", "glibc"),
        hash: "g".repeat(64),
        ..Default::default()
    };
    let lib = RegisterPathParams {
        path: make_path("llllllllllllllllllllllllllllllll", "mylib"),
        hash: "l".repeat(64),
        references: BTreeSet::from([glibc.path.clone()]),
        ..Default::default()
    };
    let app = RegisterPathParams {
        path: make_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "myapp"),
        hash: "a".repeat(64),
        references: BTreeSet::from([lib.path.clone(), glibc.path.clone()]),
        ..Default::default()
    };

    db.register_valid_path(&glibc).unwrap();
    db.register_valid_path(&lib).unwrap();
    db.register_valid_path(&app).unwrap();

    // Check forward references
    let app_refs = db.query_references(&app.path).unwrap();
    assert_eq!(app_refs.len(), 2);
    assert!(app_refs.contains(&lib.path));
    assert!(app_refs.contains(&glibc.path));

    // Check reverse references (referrers)
    let glibc_referrers = db.query_referrers(&glibc.path).unwrap();
    assert_eq!(glibc_referrers.len(), 2);
    assert!(glibc_referrers.contains(&lib.path));
    assert!(glibc_referrers.contains(&app.path));
}

/// Verify derivation output tracking.
#[test]
fn test_derivation_outputs() {
    let mut db = StoreDb::open_memory().unwrap();

    let drv = RegisterPathParams {
        path: make_path("dddddddddddddddddddddddddddddddd", "hello.drv"),
        hash: "d".repeat(64),
        ..Default::default()
    };
    let out = RegisterPathParams {
        path: make_path("oooooooooooooooooooooooooooooooo", "hello"),
        hash: "o".repeat(64),
        deriver: Some(drv.path.clone()),
        ..Default::default()
    };

    db.register_valid_path(&drv).unwrap();
    db.register_valid_path(&out).unwrap();
    db.register_derivation_output(&drv.path, "out", &out.path)
        .unwrap();

    // Query outputs from derivation
    let outputs = db.query_derivation_outputs(&drv.path).unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].output_id, "out");
    assert_eq!(outputs[0].path, out.path);

    // Query derivers from output
    let derivers = db.query_valid_derivers(&out.path).unwrap();
    assert_eq!(derivers, vec![drv.path]);
}

/// Verify path invalidation cascades correctly.
#[test]
fn test_invalidation_cascade() {
    let mut db = StoreDb::open_memory().unwrap();

    let dep = RegisterPathParams {
        path: make_path("dddddddddddddddddddddddddddddddd", "dep"),
        hash: "d".repeat(64),
        ..Default::default()
    };
    // Self-reference (allowed in Nix)
    let main = RegisterPathParams {
        path: make_path("mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm", "main"),
        hash: "m".repeat(64),
        references: BTreeSet::from([dep.path.clone()]),
        ..Default::default()
    };

    db.register_valid_path(&dep).unwrap();
    db.register_valid_path(&main).unwrap();

    // Add self-reference
    db.add_reference(&main.path, &main.path).unwrap();

    // Invalidate main - should work despite self-reference (trigger handles it)
    assert!(db.invalidate_path(&main.path).unwrap());
    assert!(!db.is_valid_path(&main.path).unwrap());

    // dep should still exist
    assert!(db.is_valid_path(&dep.path).unwrap());
}
