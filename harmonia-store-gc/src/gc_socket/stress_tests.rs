// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Concurrency stress test for the gc-socket protocol.
//!
//! It drives the real GcSocketServer with concurrent clients against a
//! deleter that mirrors gc.rs's claim/end_delete cycle, over many random
//! graphs.
//!
//! Invariant: a node must never be unlinked after it has been acked to a
//! client. An acked builder treats the path as stable and may hardlink
//! into it, so unlinking it afterwards resurrects a path the GC believes
//! it deleted. The acked node's whole closure is checked.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};

use super::{GcSocketServer, LiveSet};
use harmonia_store_db::{GraphOptions, NodeIdx, StoreDb, StoreGraph};
use harmonia_store_path::StoreDir;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn full_path(i: usize) -> String {
    format!("/nix/store/{i:032x}-pkg")
}

/// Random DAG: node i only references nodes < i, so closures are acyclic.
fn gen_graph(rng: &mut StdRng, n: usize) -> StoreGraph {
    let db = StoreDb::open_memory().unwrap();
    for i in 0..n {
        db.connection()
            .execute(
                "INSERT INTO ValidPaths (id, path, hash, registrationTime, narSize) \
                 VALUES (?, ?, 'sha256:x', 1, 0)",
                rusqlite::params![i as i64 + 1, full_path(i)],
            )
            .unwrap();
    }
    for i in 0..n {
        for j in 0..i {
            if rng.random_bool(0.25) {
                db.connection()
                    .execute(
                        "INSERT INTO Refs (referrer, reference) VALUES (?, ?)",
                        rusqlite::params![i as i64 + 1, j as i64 + 1],
                    )
                    .unwrap();
            }
        }
    }
    db.load_graph(
        &StoreDir::default(),
        &GraphOptions {
            keep_derivations: false,
            keep_outputs: false,
        },
    )
    .unwrap()
}

/// Mirror of the gc.rs deletion loop: claim a chunk, drop the closures of
/// skipped (protected) paths, then unlink each claimed node.
fn run_deleter(graph: &StoreGraph, live: &LiveSet, acked: &[AtomicBool], seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let all: Vec<NodeIdx> = graph.nodes().collect();
    let mut cursor = 0;
    while cursor < all.len() {
        let chunk: Vec<NodeIdx> = all[cursor..]
            .iter()
            .copied()
            .take(rng.random_range(1..=4))
            .collect();
        cursor += chunk.len();

        let (mut claimed, skipped) = live.claim_nodes(&chunk);
        if !skipped.is_empty() {
            let keep = graph.compute_closure(&skipped);
            claimed.retain(|&node| {
                if keep.contains(node) {
                    live.end_delete_node(node);
                }
                !keep.contains(node)
            });
        }
        for &node in &claimed {
            // Hold the node mid-unlink briefly so clients reliably race the
            // gap between claim and unlink, the window protect() must
            // synchronise on. Without it the bug only shows up by luck.
            std::thread::sleep(std::time::Duration::from_micros(50));
            assert!(
                !acked[node.index()].load(Ordering::SeqCst),
                "unlinking node {} after it was acked to a client",
                node.index(),
            );
            live.end_delete_node(node);
        }
    }
}

/// Register random roots and mark each acked root's closure. The deleter
/// must never unlink an acked node.
fn run_client(
    sock: &std::path::Path,
    graph: &StoreGraph,
    acked: &[AtomicBool],
    stop: &AtomicBool,
    seed: u64,
) {
    let mut rng = StdRng::seed_from_u64(seed);
    let all: Vec<NodeIdx> = graph.nodes().collect();
    let Ok(mut conn) = UnixStream::connect(sock) else {
        return;
    };
    while !stop.load(Ordering::Acquire) {
        let root = all[rng.random_range(0..all.len())];
        let line = format!("{}\n", graph.path(root));
        let mut ack = [0u8; 1];
        if conn.write_all(line.as_bytes()).is_err() || conn.read_exact(&mut ack).is_err() {
            return;
        }
        assert_eq!(ack, *b"1");
        let closure = graph.compute_closure(&[root]);
        for &node in &all {
            if closure.contains(node) {
                acked[node.index()].store(true, Ordering::SeqCst);
            }
        }
    }
}

fn one_run(seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    let n = rng.random_range(4..44);
    let graph = Arc::new(gen_graph(&mut rng, n));
    let live = Arc::new(LiveSet::new(n));
    let acked: Arc<Vec<AtomicBool>> = Arc::new((0..n).map(|_| AtomicBool::new(false)).collect());

    let dir = tempfile::tempdir().unwrap();
    let server = GcSocketServer::start(dir.path(), Arc::clone(&live), Arc::clone(&graph)).unwrap();
    let sock = dir.path().join("gc-socket/socket");

    let stop = Arc::new(AtomicBool::new(false));
    let n_clients = 4;
    let barrier = Arc::new(Barrier::new(n_clients + 1));

    let clients: Vec<_> = (0..n_clients)
        .map(|c| {
            let graph = Arc::clone(&graph);
            let acked = Arc::clone(&acked);
            let stop = Arc::clone(&stop);
            let barrier = Arc::clone(&barrier);
            let sock = sock.clone();
            std::thread::spawn(move || {
                barrier.wait();
                run_client(&sock, &graph, &acked, &stop, seed ^ (c as u64 + 1));
            })
        })
        .collect();

    barrier.wait();
    run_deleter(&graph, &live, &acked, seed);

    stop.store(true, Ordering::Release);
    // Dropping the server closes client connections so the threads exit.
    drop(server);
    for c in clients {
        c.join().unwrap();
    }
}

#[test]
fn concurrent_clients_never_resurrect_deleted_paths() {
    // Deterministic per seed. A protect() deadlock regression surfaces as
    // the harness timing out.
    for seed in 1..=60u64 {
        one_run(seed);
    }
}
