// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Tests for the `query_missing` handler method.

use std::sync::Arc;

use harmonia_protocol::daemon::{DaemonStore, HandshakeDaemonStore};
use harmonia_store_core::derived_path::{DerivedPath, OutputSpec, SingleDerivedPath};
use harmonia_store_core::store_path::StorePath;

use super::test_store::TestStore;

/// `DerivedPath::Opaque` not in store → appears in `unknown`.
/// `DerivedPath::Opaque` present in store → absent from all result sets.
///
/// Tests the core classification logic: the Opaque variant only checks
/// local validity, never triggers will_build or will_substitute.
#[tokio::test]
async fn test_query_missing_opaque_classification() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    // Register one path as present
    let present = StorePath::from_base_path("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-present").unwrap();
    let present_full = format!("{}/{}", ts.store_dir, present);
    let disk = ts.store_path().join(present.to_string());
    std::fs::write(&disk, "content").unwrap();
    {
        let mut db = ts.db.lock().await;
        db.register_valid_path(&harmonia_store_db::RegisterPathParams {
            path: present_full,
            hash: "sha256:0000000000000000000000000000000000000000000000000000000000000001".into(),
            ..Default::default()
        })
        .unwrap();
    }

    let missing = StorePath::from_base_path("mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm-gone").unwrap();

    let paths = vec![
        DerivedPath::Opaque(present.clone()),
        DerivedPath::Opaque(missing.clone()),
    ];
    let result = store.query_missing(&paths).await.unwrap();

    // Present path should not appear anywhere
    assert!(!result.unknown.contains(&present));
    assert!(!result.will_build.contains(&present));
    assert!(!result.will_substitute.contains(&present));

    // Missing path should be in unknown (no substituters configured)
    assert!(
        result.unknown.contains(&missing),
        "Missing opaque path should be in unknown"
    );
}

/// `DerivedPath::Built` with drv not in store → `unknown` (can't build
/// what we don't have). With drv in store → `will_build` (outputs need
/// building).
#[tokio::test]
async fn test_query_missing_built_classification() {
    let ts = TestStore::new();
    let mut store = ts.handler.clone().handshake().await.unwrap();

    // Register a derivation path as present (we only check validity, not
    // actual .drv contents)
    let known_drv =
        StorePath::from_base_path("kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk-known.drv").unwrap();
    let known_full = format!("{}/{}", ts.store_dir, known_drv);
    let disk = ts.store_path().join(known_drv.to_string());
    std::fs::write(&disk, "Derive(...)").unwrap();
    {
        let mut db = ts.db.lock().await;
        db.register_valid_path(&harmonia_store_db::RegisterPathParams {
            path: known_full,
            hash: "sha256:0000000000000000000000000000000000000000000000000000000000000002".into(),
            ..Default::default()
        })
        .unwrap();
    }

    let unknown_drv =
        StorePath::from_base_path("dddddddddddddddddddddddddddddddd-missing.drv").unwrap();

    let paths = vec![
        DerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(known_drv.clone())),
            outputs: OutputSpec::All,
        },
        DerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(unknown_drv.clone())),
            outputs: OutputSpec::All,
        },
    ];

    let result = store.query_missing(&paths).await.unwrap();

    // Known drv → will_build (outputs not checked yet)
    assert!(
        result.will_build.contains(&known_drv),
        "Known derivation should appear in will_build"
    );

    // Unknown drv → unknown (can't build what we don't have)
    assert!(
        result.unknown.contains(&unknown_drv),
        "Unknown derivation should appear in unknown, not will_build"
    );
    assert!(
        !result.will_build.contains(&unknown_drv),
        "Unknown derivation should NOT appear in will_build"
    );
}
