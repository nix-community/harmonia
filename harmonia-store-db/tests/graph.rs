// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Tests for the reference graph loader and bulk invalidation.

use harmonia_store_db::{BasenameIndex, GraphOptions, NodeIdx, StoreDb, StoreGraph};
use harmonia_store_path::StoreDir;

const H1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const H2: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const H3: &str = "cccccccccccccccccccccccccccccccc";

const NO_KEEP: GraphOptions = GraphOptions {
    keep_derivations: false,
    keep_outputs: false,
};

fn full(name: &str) -> String {
    format!("/nix/store/{name}")
}

fn setup() -> (StoreDb, StoreDir) {
    (StoreDb::open_memory().unwrap(), StoreDir::default())
}

fn add_path(db: &StoreDb, name: &str, nar_size: i64, reg_time: i64) -> i64 {
    db.connection()
        .execute(
            "INSERT INTO ValidPaths (path, hash, registrationTime, narSize) \
             VALUES (?, 'sha256:x', ?, ?)",
            rusqlite::params![full(name), reg_time, nar_size],
        )
        .unwrap();
    db.connection().last_insert_rowid()
}

fn add_ref(db: &StoreDb, referrer: i64, reference: i64) {
    db.connection()
        .execute(
            "INSERT INTO Refs (referrer, reference) VALUES (?, ?)",
            rusqlite::params![referrer, reference],
        )
        .unwrap();
}

fn idx_of(g: &StoreGraph, suffix: &str) -> NodeIdx {
    g.nodes().find(|&n| g.path(n).ends_with(suffix)).unwrap()
}

fn refs_of(g: &StoreGraph, node: NodeIdx) -> Vec<NodeIdx> {
    let mut refs: Vec<NodeIdx> = g.refs(node).collect();
    refs.sort();
    refs
}

#[test]
fn load_graph_builds_csr() {
    let (db, sd) = setup();
    let a = add_path(&db, &format!("{H1}-a"), 100, 1000);
    let b = add_path(&db, &format!("{H2}-b"), 200, 2000);
    let c = add_path(&db, &format!("{H3}-c"), 300, 3000);
    // Two edges from one node exercise the CSR cursor advance.
    add_ref(&db, a, b);
    add_ref(&db, a, c);

    let g = db.load_graph(&sd, &NO_KEEP).unwrap();
    assert_eq!(g.len(), 3);
    assert!(!g.is_empty());
    let (ia, ib, ic) = (idx_of(&g, "-a"), idx_of(&g, "-b"), idx_of(&g, "-c"));
    assert_eq!(g.nar_size(ia), 100);
    assert_eq!(g.registration_time(ic), 3000);
    let mut expected = vec![ib, ic];
    expected.sort();
    assert_eq!(refs_of(&g, ia), expected);
    assert_eq!(g.refs(ib).len(), 0);
    assert_eq!(g.refs(ic).len(), 0);
}

#[test]
fn empty_graph() {
    let (db, sd) = setup();
    let g = db.load_graph(&sd, &NO_KEEP).unwrap();
    assert_eq!(g.len(), 0);
    assert!(g.is_empty());
    assert_eq!(g.store_prefix(), "/nix/store/");
}

#[test]
fn store_prefix_strips_trailing_slash() {
    let (db, _) = setup();
    let sd = StoreDir::new("/nix/store/").unwrap();
    let g = db.load_graph(&sd, &NO_KEEP).unwrap();
    assert_eq!(g.store_prefix(), "/nix/store/");
}

#[test]
fn load_graph_ignores_edges_with_unknown_ids() {
    let (db, sd) = setup();
    let a = add_path(&db, &format!("{H1}-a"), 100, 1000);
    // Dangling edges must be dropped, not crash or corrupt the CSR.
    db.connection()
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    db.connection()
        .execute_batch(&format!("INSERT INTO Refs VALUES ({a}, 9999), (9999, {a})"))
        .unwrap();
    let g = db.load_graph(&sd, &NO_KEEP).unwrap();
    let node = g.nodes().next().unwrap();
    assert_eq!(g.refs(node).len(), 0);
}

#[test]
fn load_graph_keep_derivations_adds_output_to_drv_edge() {
    let (db, sd) = setup();
    add_path(&db, &format!("{H1}-pkg.drv"), 10, 1000);
    let out = add_path(&db, &format!("{H2}-pkg"), 100, 1000);
    db.connection()
        .execute(
            "UPDATE ValidPaths SET deriver = ? WHERE id = ?",
            rusqlite::params![full(&format!("{H1}-pkg.drv")), out],
        )
        .unwrap();

    let g = db
        .load_graph(
            &sd,
            &GraphOptions {
                keep_derivations: true,
                keep_outputs: false,
            },
        )
        .unwrap();
    let (iout, idrv) = (idx_of(&g, "-pkg"), idx_of(&g, ".drv"));
    assert_eq!(refs_of(&g, iout), vec![idrv]);
    assert_eq!(g.refs(idrv).len(), 0);

    // keep-outputs: the deriver field alone pins the output (it is the only
    // drv→output mapping for Realisations-era CA outputs).
    let g = db
        .load_graph(
            &sd,
            &GraphOptions {
                keep_derivations: true,
                keep_outputs: true,
            },
        )
        .unwrap();
    let (iout, idrv) = (idx_of(&g, "-pkg"), idx_of(&g, ".drv"));
    assert_eq!(refs_of(&g, idrv), vec![iout]);
}

#[test]
fn load_graph_build_trace_edges() {
    let (db, sd) = setup();
    let drv_base = format!("{H1}-pkg.drv");
    add_path(&db, &drv_base, 10, 1000);
    add_path(&db, &format!("{H2}-pkg"), 100, 1000);
    db.connection()
        .execute(
            "INSERT INTO BuildTraceV3 (drvPath, outputName, outputPath) VALUES (?, 'out', ?)",
            rusqlite::params![drv_base, full(&format!("{H2}-pkg"))],
        )
        .unwrap();

    // keep-derivations pins drvs only via the deriver field. A BuildTraceV3
    // row alone must not create an output→drv edge (the output may have been
    // rebuilt by a newer drv).
    let g = db
        .load_graph(
            &sd,
            &GraphOptions {
                keep_derivations: true,
                keep_outputs: false,
            },
        )
        .unwrap();
    let (iout, _idrv) = (idx_of(&g, "-pkg"), idx_of(&g, ".drv"));
    assert_eq!(g.refs(iout).len(), 0);

    // keep-outputs pins CA outputs of an alive drv via BuildTraceV3.
    let g = db
        .load_graph(
            &sd,
            &GraphOptions {
                keep_derivations: false,
                keep_outputs: true,
            },
        )
        .unwrap();
    let (iout, idrv) = (idx_of(&g, "-pkg"), idx_of(&g, ".drv"));
    assert_eq!(refs_of(&g, idrv), vec![iout]);
}

#[test]
fn basename_index_lookups() {
    let (db, sd) = setup();
    add_path(&db, &format!("{H1}-a"), 1, 1);
    add_path(&db, &format!("{H2}-b"), 1, 1);
    add_path(&db, &format!("{H3}-c"), 1, 1);
    let g = db.load_graph(&sd, &NO_KEEP).unwrap();
    let bidx = BasenameIndex::new(&g);

    let ic = idx_of(&g, "-c");
    assert_eq!(bidx.get(&full(&format!("{H3}-c"))), Some(ic));
    assert_eq!(bidx.get_basename(&format!("{H3}-c")), Some(ic));
    assert_eq!(bidx.get("/elsewhere/x"), None);
    assert_eq!(bidx.get_basename("nope"), None);
}

#[test]
fn compute_closure_marks_reachable_only() {
    let (db, sd) = setup();
    let a = add_path(&db, &format!("{H1}-a"), 1, 1);
    let b = add_path(&db, &format!("{H2}-b"), 1, 1);
    add_path(&db, &format!("{H3}-c"), 1, 1);
    add_ref(&db, a, b);
    // Cycle must terminate.
    add_ref(&db, b, a);

    let g = db.load_graph(&sd, &NO_KEEP).unwrap();
    let (ia, ic) = (idx_of(&g, "-a"), idx_of(&g, "-c"));
    // Duplicate roots must not double-visit.
    let closure = g.compute_closure(&[ia, ia]);
    assert_eq!(closure.len(), 2);
    assert!(!closure.is_empty());
    assert!(closure.contains(ia));
    assert!(!closure.contains(ic));
}

#[test]
fn empty_store_graph_placeholder() {
    let g = StoreGraph::empty("/nix/store/".into());
    assert!(g.is_empty());
    assert!(g.compute_closure(&[]).is_empty());
    assert_eq!(g.nodes().len(), 0);
}

#[test]
fn load_graph_handles_sparse_ids() {
    let (db, sd) = setup();
    let a = add_path(&db, &format!("{H1}-a"), 100, 1000);
    db.connection()
        .execute(
            "INSERT INTO ValidPaths (id, path, hash, registrationTime, narSize) \
             VALUES (?, ?, 'sha256:x', 1000, 200)",
            rusqlite::params![1_000_000_000_000i64, full(&format!("{H2}-b"))],
        )
        .unwrap();
    add_ref(&db, 1_000_000_000_000, a);

    let g = db.load_graph(&sd, &NO_KEEP).unwrap();
    assert_eq!(g.len(), 2);
    let (ia, ib) = (idx_of(&g, "-a"), idx_of(&g, "-b"));
    assert_eq!(refs_of(&g, ib), vec![ia]);
}

#[test]
fn load_graph_dedups_keep_outputs_edges() {
    let (db, sd) = setup();
    let drv_base = format!("{H1}-pkg.drv");
    let drv = add_path(&db, &drv_base, 10, 1000);
    let out = add_path(&db, &format!("{H2}-pkg"), 100, 1000);
    db.connection()
        .execute(
            "UPDATE ValidPaths SET deriver = ? WHERE id = ?",
            rusqlite::params![full(&drv_base), out],
        )
        .unwrap();
    db.connection()
        .execute(
            "INSERT INTO DerivationOutputs (drv, id, path) VALUES (?, 'out', ?)",
            rusqlite::params![drv, full(&format!("{H2}-pkg"))],
        )
        .unwrap();

    let g = db
        .load_graph(
            &sd,
            &GraphOptions {
                keep_derivations: false,
                keep_outputs: true,
            },
        )
        .unwrap();
    let (idrv, iout) = (idx_of(&g, ".drv"), idx_of(&g, "-pkg"));
    assert_eq!(refs_of(&g, idrv), vec![iout]);
}
