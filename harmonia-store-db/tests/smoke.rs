// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Smoke tests for harmonia-store-db.
//!
//! These tests verify the schema and basic operations work correctly
//! using an in-memory database.

use std::collections::BTreeSet;

use harmonia_store_db::StoreDb;
use harmonia_store_path::{StoreDir, StorePath, StorePathHash};
use harmonia_store_path_info::{NarHash, UnkeyedValidPathInfo};
use harmonia_utils_signature::Signature;

fn sd() -> StoreDir {
    StoreDir::default()
}

/// Create a `StorePath` from a name, hashing the name to produce a unique
/// hash part (like Hydra's test helper).
fn sp(name: &str) -> StorePath {
    let digest = harmonia_utils_hash::Algorithm::SHA256.digest(name);
    let sha = harmonia_utils_hash::Sha256::from_slice(&digest).unwrap();
    StorePath::from_hash(&sha, name.parse().unwrap())
}

/// A zero NarHash for tests.
fn zero_nar_hash() -> NarHash {
    NarHash::from_slice(&[0u8; 32]).unwrap()
}

fn test_sig() -> Signature {
    Signature::from_parts("test", &[0u8; 64]).unwrap()
}

fn empty_info() -> UnkeyedValidPathInfo {
    UnkeyedValidPathInfo {
        deriver: None,
        nar_hash: zero_nar_hash(),
        references: BTreeSet::new(),
        registration_time: None,
        nar_size: 0,
        ultimate: false,
        signatures: BTreeSet::new(),
        ca: None,
        store_dir: sd(),
    }
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

    let path = sp("hello");
    let info = UnkeyedValidPathInfo {
        nar_size: 12345,
        ultimate: true,
        signatures: [test_sig()].into_iter().collect(),
        ..empty_info()
    };

    let id = db.register_valid_path(&sd(), &path, &info).unwrap();
    assert!(id > 0);

    let result = db.query_path_info(&sd(), &path).unwrap().unwrap();
    assert_eq!(result.path, path);
    assert_eq!(result.info.nar_size, 12345);
    assert!(result.info.ultimate);
    assert!(!result.info.signatures.is_empty());
}

/// `query_path_info_by_hash_part` must not return the lexicographic neighbour
/// when the requested hash is absent, and must include references on a hit.
#[test]
fn test_query_path_info_by_hash_part() {
    let mut db = StoreDb::open_memory().unwrap();

    let dep = sp("dep");
    let pkg = sp("hello");

    db.register_valid_path(
        &sd(),
        &dep,
        &UnkeyedValidPathInfo {
            nar_size: 1,
            ..empty_info()
        },
    )
    .unwrap();
    db.register_valid_path(
        &sd(),
        &pkg,
        &UnkeyedValidPathInfo {
            nar_size: 42,
            references: BTreeSet::from([dep.clone()]),
            ..empty_info()
        },
    )
    .unwrap();

    // Hit: exact hash part returns the row plus its references.
    let info = db
        .query_path_info_by_hash_part(&sd(), pkg.hash())
        .unwrap()
        .unwrap();
    assert_eq!(info.path, pkg);
    assert_eq!(info.info.nar_size, 42);
    assert!(info.info.references.contains(&dep));

    // Miss: a hash part that sorts before an existing entry must not leak it.
    let miss_hash = StorePathHash::new([0u8; 20]);
    let miss = db.query_path_info_by_hash_part(&sd(), &miss_hash).unwrap();
    assert!(miss.is_none());
}

/// Verify reference graph operations.
#[test]
fn test_reference_graph() {
    let mut db = StoreDb::open_memory().unwrap();

    // Create a dependency chain: app -> lib -> glibc
    let glibc = sp("glibc");
    let lib = sp("mylib");
    let app = sp("myapp");

    db.register_valid_path(&sd(), &glibc, &empty_info())
        .unwrap();
    db.register_valid_path(
        &sd(),
        &lib,
        &UnkeyedValidPathInfo {
            references: BTreeSet::from([glibc.clone()]),
            ..empty_info()
        },
    )
    .unwrap();
    db.register_valid_path(
        &sd(),
        &app,
        &UnkeyedValidPathInfo {
            references: BTreeSet::from([lib.clone(), glibc.clone()]),
            ..empty_info()
        },
    )
    .unwrap();

    // Check forward references
    let app_refs = db.query_references(&sd(), &app).unwrap();
    assert_eq!(app_refs.len(), 2);
    assert!(app_refs.contains(&lib));
    assert!(app_refs.contains(&glibc));

    // Check reverse references (referrers)
    let glibc_referrers = db.query_referrers(&sd(), &glibc).unwrap();
    assert_eq!(glibc_referrers.len(), 2);
    assert!(glibc_referrers.contains(&lib));
    assert!(glibc_referrers.contains(&app));
}

/// Verify derivation output tracking.
#[test]
fn test_derivation_outputs() {
    let mut db = StoreDb::open_memory().unwrap();

    let drv = sp("hello.drv");
    let out = sp("hello");
    let out_name: harmonia_store_core::derived_path::OutputName = "out".parse().unwrap();

    db.register_valid_path(&sd(), &drv, &empty_info()).unwrap();
    db.register_valid_path(
        &sd(),
        &out,
        &UnkeyedValidPathInfo {
            deriver: Some(drv.clone()),
            ..empty_info()
        },
    )
    .unwrap();
    db.register_derivation_output(&sd(), &drv, &out_name, &out)
        .unwrap();

    // Query outputs from derivation
    let outputs = db.query_derivation_outputs(&sd(), &drv).unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].output_id.as_ref(), "out");
    assert_eq!(outputs[0].path, out);

    // Query derivers from output
    let derivers = db.query_valid_derivers(&sd(), &out).unwrap();
    assert_eq!(derivers, vec![drv]);
}

/// Verify CA realisations roundtrip through the BuildTraceV3 table.
#[test]
fn test_realisation_roundtrip() {
    let db = StoreDb::open_memory().unwrap();

    let drv_path = sp("hello.drv");
    let out_path = sp("hello");
    let output_name: harmonia_store_core::derived_path::OutputName = "out".parse().unwrap();

    assert!(
        db.query_realisation(&sd(), &drv_path, &output_name)
            .unwrap()
            .is_none()
    );

    let realisation = harmonia_store_core::realisation::Realisation {
        key: harmonia_store_core::realisation::DrvOutput {
            drv_path: drv_path.clone(),
            output_name: output_name.clone(),
        },
        value: harmonia_store_core::realisation::UnkeyedRealisation {
            out_path: out_path.clone(),
            signatures: [test_sig()].into_iter().collect(),
        },
    };
    db.register_realisation(&sd(), &realisation).unwrap();

    let r = db
        .query_realisation(&sd(), &drv_path, &output_name)
        .unwrap()
        .unwrap();
    assert_eq!(r.realisation.key.drv_path, drv_path);
    assert_eq!(r.realisation.key.output_name.as_ref(), "out");
    assert_eq!(r.realisation.value.out_path, out_path);
    assert_eq!(r.realisation.value.signatures.len(), 1);

    let dev_name: harmonia_store_core::derived_path::OutputName = "dev".parse().unwrap();
    assert!(
        db.query_realisation(&sd(), &drv_path, &dev_name)
            .unwrap()
            .is_none()
    );
}

/// Nix stores `drvPath` as a base path but `outputPath` as a full path
/// (with `/nix/store/` prefix) in `BuildTraceV3`. Verify we can read
/// realisations written that way.
#[test]
fn test_realisation_nix_compat() {
    let db = StoreDb::open_memory().unwrap();

    let drv_path = sp("hello.drv");
    let out_path = sp("hello");
    let output_name: harmonia_store_core::derived_path::OutputName = "out".parse().unwrap();

    // Simulate what Nix C++ does: base path for drvPath, full path for outputPath.
    db.connection().execute(
        "INSERT INTO BuildTraceV3 (drvPath, outputName, outputPath, signatures) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            drv_path.to_string(),
            "out",
            format!("{}/{}", sd(), out_path),
            "test:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ],
    ).unwrap();

    let r = db
        .query_realisation(&sd(), &drv_path, &output_name)
        .unwrap()
        .unwrap();
    assert_eq!(r.realisation.key.drv_path, drv_path);
    assert_eq!(r.realisation.value.out_path, out_path);
}

/// Verify path invalidation cascades correctly.
#[test]
fn test_invalidation_cascade() {
    let mut db = StoreDb::open_memory().unwrap();

    let dep = sp("dep");
    let main = sp("main");

    db.register_valid_path(&sd(), &dep, &empty_info()).unwrap();
    db.register_valid_path(
        &sd(),
        &main,
        &UnkeyedValidPathInfo {
            references: BTreeSet::from([dep.clone()]),
            ..empty_info()
        },
    )
    .unwrap();

    // Add self-reference
    db.add_reference(&sd(), &main, &main).unwrap();

    // Invalidate main - should work despite self-reference (trigger handles it)
    assert!(db.invalidate_path(&sd(), &main).unwrap());
    assert!(!db.is_valid_path(&sd(), &main).unwrap());

    // dep should still exist
    assert!(db.is_valid_path(&sd(), &dep).unwrap());
}
