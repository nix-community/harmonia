// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! GC roots socket, protocol-compatible with `nix-store --gc`.
//!
//! Builders that fail to acquire a shared `gc.lock` connect to
//! `state/gc-socket/socket`, send newline-terminated store paths, and
//! wait for a single `'1'` ack per line. We mark the closure protected
//! before acking so the builder cannot recreate a path we are still
//! unlinking.

use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use harmonia_store_db::{NodeIdx, StoreGraph};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::{HashMap, HashSet};

/// Liveness state shared between the deletion loop and the socket server.
///
/// Roots from clients can only flip dead -> protected. `pending` tracks
/// in-flight unlinks so `protect()` waits for them before acking.
pub(crate) struct LiveSet {
    inner: Mutex<LiveInner>,
    cond: Condvar,
}

struct LiveInner {
    /// GC teardown in progress. protect() must stop waiting.
    cancelled: bool,
    /// Per-node "do not delete" flag.
    protected: Vec<bool>,
    /// Protected basenames not (yet) in the graph, e.g. paths registered
    /// after our database snapshot.
    protected_unknown: HashSet<String>,
    /// Graph nodes currently being deleted.
    pending_nodes: HashSet<NodeIdx>,
    /// Unknown-on-disk basenames currently being deleted.
    pending_unknown: HashSet<String>,
}

impl LiveSet {
    pub(crate) fn new(n_nodes: usize) -> Self {
        LiveSet {
            inner: Mutex::new(LiveInner {
                cancelled: false,
                protected: vec![false; n_nodes],
                protected_unknown: HashSet::default(),
                pending_nodes: HashSet::default(),
                pending_unknown: HashSet::default(),
            }),
            cond: Condvar::new(),
        }
    }

    pub(crate) fn end_delete_node(&self, node: NodeIdx) {
        let mut g = self.inner.lock().unwrap();
        g.pending_nodes.remove(&node);
        drop(g);
        self.cond.notify_all();
    }

    /// Snapshot of per-node protection flags (dry-run reporting).
    pub(crate) fn protected_snapshot(&self) -> Vec<bool> {
        self.inner.lock().unwrap().protected.clone()
    }

    /// Snapshot of protected basenames that are not in the graph.
    pub(crate) fn protected_unknown_snapshot(&self) -> HashSet<String> {
        self.inner.lock().unwrap().protected_unknown.clone()
    }

    /// Mark a basename outside the graph as protected. Used to carry
    /// over roots received before the graph was loaded.
    pub(crate) fn protect_unknown_basename(&self, basename: &str) {
        self.inner
            .lock()
            .unwrap()
            .protected_unknown
            .insert(basename.to_owned());
    }

    /// Atomically partition `nodes` into (claimed, skipped). Skipped
    /// nodes are protected. Claimed ones are marked pending, so a later
    /// `protect()` blocks until their unlink finished. That is what keeps
    /// a builder from being acked while the row deletion is in flight.
    pub(crate) fn claim_nodes(&self, nodes: &[NodeIdx]) -> (Vec<NodeIdx>, Vec<NodeIdx>) {
        let mut g = self.inner.lock().unwrap();
        let mut claimed = Vec::with_capacity(nodes.len());
        let mut skipped = Vec::new();
        for &n in nodes {
            if g.protected[n.index()] {
                skipped.push(n);
            } else {
                g.pending_nodes.insert(n);
                claimed.push(n);
            }
        }
        (claimed, skipped)
    }

    /// Atomically check that the path is unprotected and mark it
    /// pending, for paths not in the graph.
    pub(crate) fn try_begin_delete_unknown(&self, basename: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.protected_unknown.contains(basename) {
            return false;
        }
        g.pending_unknown.insert(basename.to_owned());
        true
    }

    pub(crate) fn end_delete_unknown(&self, basename: &str) {
        let mut g = self.inner.lock().unwrap();
        g.pending_unknown.remove(basename);
        drop(g);
        self.cond.notify_all();
    }

    /// Mark the closure of `basename` as protected and wait until none of
    /// it is still being deleted.
    /// Returns `false` if cancelled by GC teardown; the caller must not
    /// ack then.
    fn protect(&self, basename: &str, graph: &StoreGraph, idx: &HashMap<String, NodeIdx>) -> bool {
        let mut g = self.inner.lock().unwrap();
        // Wait for every pending closure node, including ones an earlier
        // overlapping protect() already marked. Acking before their
        // unlinks finish would let the client recreate a path mid-delete.
        let mut wait_for: Vec<NodeIdx> = Vec::new();
        if let Some(&root) = idx.get(basename) {
            let mut stack = vec![root];
            let mut seen: HashSet<NodeIdx> = HashSet::default();
            while let Some(n) = stack.pop() {
                if !seen.insert(n) {
                    continue;
                }
                g.protected[n.index()] = true;
                if g.pending_nodes.contains(&n) {
                    wait_for.push(n);
                }
                stack.extend(graph.refs(n));
            }
        } else {
            g.protected_unknown.insert(basename.to_owned());
        }

        let conflict = |g: &LiveInner| {
            wait_for.iter().any(|n| g.pending_nodes.contains(n))
                || g.pending_unknown.contains(basename)
        };
        while conflict(&g) && !g.cancelled {
            debug!("synchronising with deletion of {basename}");
            g = self.cond.wait(g).unwrap();
        }
        !g.cancelled
    }

    /// Unblock every waiting protect(). Used on GC teardown, where an
    /// error path may leave pending nodes that will never finish.
    fn cancel(&self) {
        self.inner.lock().unwrap().cancelled = true;
        self.cond.notify_all();
    }
}

/// Client connections and their handler threads, owned by the server.
type Conns = Arc<Mutex<Vec<(UnixStream, JoinHandle<()>)>>>;

/// Running GC roots socket server.
///
/// Dropping it tears down the listener, removes the socket file, and
/// joins the accept thread and every client handler.
pub(crate) struct GcSocketServer {
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    live: Arc<LiveSet>,
    /// Live client connections. Drop shuts them down and joins their
    /// handler threads so no `protect()` is still running afterwards.
    /// Callers snapshot the LiveSet right after dropping the server.
    conns: Conns,
}

impl GcSocketServer {
    /// Bind `state_dir/gc-socket/socket` and spawn the accept thread.
    pub(crate) fn start(
        state_dir: &Path,
        live: Arc<LiveSet>,
        graph: Arc<StoreGraph>,
    ) -> Result<Self> {
        let dir = state_dir.join("gc-socket");
        fs::create_dir_all(&dir).map_err(|source| Error::GcSocketServe {
            path: dir.clone(),
            source,
        })?;
        let socket_path = dir.join("socket");
        let serve_err = |source| Error::GcSocketServe {
            path: socket_path.clone(),
            source,
        };
        // A previous GC may have crashed without cleaning up.
        let _ = fs::remove_file(&socket_path);
        let listener = UnixListener::bind(&socket_path).map_err(serve_err)?;
        // Builders may run as a different user. Mirror Nix's 0666.
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666)).map_err(serve_err)?;

        // Owned map: BasenameIndex borrows from the graph and cannot be
        // sent across threads alongside an Arc<StoreGraph>.
        let mut idx: HashMap<String, NodeIdx> = HashMap::default();
        idx.reserve(graph.len());
        for node in graph.nodes() {
            if let Some(b) = graph.path(node).strip_prefix(graph.store_prefix()) {
                idx.insert(b.to_owned(), node);
            }
        }
        let idx = Arc::new(idx);
        let store_prefix = graph.store_prefix().to_owned();
        let live_for_drop = Arc::clone(&live);
        let shutdown = Arc::new(AtomicBool::new(false));
        let accept_shutdown = Arc::clone(&shutdown);
        let conns: Conns = Arc::new(Mutex::new(Vec::new()));
        let accept_conns = Arc::clone(&conns);

        let handle = thread::Builder::new()
            .name("gc-socket".into())
            .spawn(move || {
                accept_loop(
                    listener,
                    store_prefix,
                    live,
                    graph,
                    idx,
                    accept_shutdown,
                    accept_conns,
                )
            })
            .map_err(|source| Error::SpawnThread {
                name: "gc-socket",
                source,
            })?;

        Ok(GcSocketServer {
            socket_path,
            shutdown,
            handle: Some(handle),
            live: live_for_drop,
            conns,
        })
    }
}

impl Drop for GcSocketServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
        self.shutdown.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        // Cancel first so a protect() stuck on abandoned pending nodes
        // cannot deadlock the join below. Unacked clients reconnect via
        // Nix's addTempRoot restart loop.
        self.live.cancel();
        let conns = std::mem::take(&mut *self.conns.lock().unwrap());
        for (stream, handle) in conns {
            let _ = stream.shutdown(Shutdown::Both);
            let _ = handle.join();
        }
    }
}

fn accept_loop(
    listener: UnixListener,
    store_prefix: String,
    live: Arc<LiveSet>,
    graph: Arc<StoreGraph>,
    idx: Arc<HashMap<String, NodeIdx>>,
    shutdown: Arc<AtomicBool>,
    conns: Conns,
) {
    while !shutdown.load(Ordering::Acquire) {
        let pfd = PollFd::new(listener.as_fd(), PollFlags::POLLIN);
        match poll(&mut [pfd], PollTimeout::from(10_u8)) {
            Ok(0) => continue,
            Err(e) => {
                // Don't hot-spin if poll fails persistently.
                debug!("gc-socket poll: {e}");
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            _ => {}
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let store_prefix = store_prefix.clone();
                let live = Arc::clone(&live);
                let graph = Arc::clone(&graph);
                let idx = Arc::clone(&idx);
                let Ok(stream2) = stream.try_clone() else {
                    continue;
                };
                let spawned =
                    thread::Builder::new()
                        .name("gc-socket-conn".into())
                        .spawn(move || {
                            if let Err(e) =
                                handle_client(stream, &store_prefix, &live, &graph, &idx)
                            {
                                debug!("gc-socket client: {e}");
                            }
                        });
                if let Ok(handle) = spawned {
                    let mut g = conns.lock().unwrap();
                    // Keep the list from growing with finished handlers.
                    g.retain(|(_, h)| !h.is_finished());
                    g.push((stream2, handle));
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => {
                // Transient failures (EMFILE/ECONNABORTED) must not kill
                // the server. Back off briefly and keep accepting.
                warn!("gc-socket accept: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn handle_client(
    stream: UnixStream,
    store_prefix: &str,
    live: &LiveSet,
    graph: &StoreGraph,
    idx: &HashMap<String, NodeIdx>,
) -> Result<()> {
    // The socket is world-writable. A peer streaming data without a
    // newline must not OOM the GC.
    const MAX_LINE: u64 = 64 * 1024;
    let client_err = |source| Error::ClientIo { source };
    let mut writer = stream.try_clone().map_err(client_err)?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .by_ref()
            .take(MAX_LINE)
            .read_line(&mut line)
            .map_err(client_err)?;
        if n == 0 {
            return Ok(());
        }
        if n as u64 == MAX_LINE && !line.ends_with('\n') {
            return Err(Error::LineTooLong);
        }
        let path = line.trim_end_matches('\n');
        if let Some(basename) = path.strip_prefix(store_prefix).filter(|b| !b.is_empty()) {
            debug!("got new GC root '{path}'");
            if !live.protect(basename, graph, idx) {
                return Ok(());
            }
        } else {
            warn!("gc-socket: received garbage instead of a root: {path:?}");
        }
        writer.write_all(b"1").map_err(client_err)?;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::testutil::graph;

    /// Single-node claim, as the deleter does per chunk via claim_nodes.
    fn claim(live: &LiveSet, n: NodeIdx) -> bool {
        let (claimed, _) = live.claim_nodes(&[n]);
        !claimed.is_empty()
    }

    fn node(g: &StoreGraph, name: &str) -> NodeIdx {
        g.nodes()
            .find(|&n| g.path(n) == format!("/nix/store/{name}"))
            .unwrap()
    }

    #[test]
    fn drop_returns_when_idle_accept_loop_is_waiting() {
        let g = graph(&[]);
        let live = Arc::new(LiveSet::new(0));
        let dir = tempfile::tempdir().unwrap();
        let server = GcSocketServer::start(dir.path(), live, g).unwrap();

        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            drop(server);
            let _ = tx.send(());
        });

        rx.recv_timeout(Duration::from_secs(2))
            .expect("GcSocketServer::drop did not finish");
        handle.join().unwrap();
    }

    #[test]
    fn protect_marks_closure_and_blocks_deletion() {
        // a -> b -> c, d standalone
        let g = graph(&[("a", &[1]), ("b", &[2]), ("c", &[]), ("d", &[])]);
        let live = Arc::new(LiveSet::new(g.len()));
        let dir = tempfile::tempdir().unwrap();
        let server = GcSocketServer::start(dir.path(), Arc::clone(&live), Arc::clone(&g)).unwrap();
        let sock = dir.path().join("gc-socket/socket");

        let mut conn = UnixStream::connect(&sock).unwrap();
        conn.write_all(b"/nix/store/a\n").unwrap();
        let mut ack = [0u8; 1];
        conn.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [b'1']);

        // The closure of a (a, b, c) is protected. d is still deletable.
        assert!(!claim(&live, node(&g, "a")));
        assert!(!claim(&live, node(&g, "b")));
        assert!(!claim(&live, node(&g, "c")));
        assert!(claim(&live, node(&g, "d")));
        live.end_delete_node(node(&g, "d"));

        drop(server);
        assert!(!sock.exists());
    }

    #[test]
    fn protect_blocks_until_pending_delete_finishes() {
        let g = graph(&[("x", &[])]);
        let live = Arc::new(LiveSet::new(g.len()));
        let dir = tempfile::tempdir().unwrap();
        let _server = GcSocketServer::start(dir.path(), Arc::clone(&live), Arc::clone(&g)).unwrap();
        let sock = dir.path().join("gc-socket/socket");

        // Simulate the deleter claiming x.
        let x = node(&g, "x");
        assert!(claim(&live, x));

        let mut conn = UnixStream::connect(&sock).unwrap();
        conn.write_all(b"/nix/store/x\n").unwrap();
        // The ack must not arrive until end_delete_node is called.
        conn.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut ack = [0u8; 1];
        assert!(conn.read_exact(&mut ack).is_err(), "ack arrived too early");

        live.end_delete_node(x);
        conn.set_read_timeout(None).unwrap();
        conn.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [b'1']);
        // After protection, a fresh delete attempt is refused.
        assert!(!claim(&live, x));
    }

    #[test]
    fn cancelled_protect_is_not_acked() {
        let g = graph(&[("x", &[])]);
        let live = Arc::new(LiveSet::new(g.len()));
        let dir = tempfile::tempdir().unwrap();
        let server = GcSocketServer::start(dir.path(), Arc::clone(&live), Arc::clone(&g)).unwrap();
        let sock = dir.path().join("gc-socket/socket");

        let x = node(&g, "x");
        assert!(claim(&live, x));
        let mut conn = UnixStream::connect(&sock).unwrap();
        conn.write_all(b"/nix/store/x\n").unwrap();
        conn.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut ack = [0u8; 1];
        assert!(conn.read_exact(&mut ack).is_err(), "ack arrived too early");

        // GC bails out with x still pending. Cancel with the server still
        // up so the outcome does not race the connection shutdown.
        live.cancel();
        assert!(
            conn.read_exact(&mut ack).is_err(),
            "cancelled protect must not be acked"
        );
        drop(server);
        let mut rest = Vec::new();
        conn.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
    }

    #[test]
    fn protect_waits_for_already_protected_pending_node() {
        // Two roots a -> c and b -> c share a leaf. While c is mid-unlink,
        // protecting a marks c protected and waits. A second protect for b
        // must also wait for c, not ack early just because c is already
        // protected.
        let g = graph(&[("a", &[2]), ("b", &[2]), ("c", &[])]);
        let live = Arc::new(LiveSet::new(g.len()));
        let dir = tempfile::tempdir().unwrap();
        let _server = GcSocketServer::start(dir.path(), Arc::clone(&live), Arc::clone(&g)).unwrap();
        let sock = dir.path().join("gc-socket/socket");

        // c is in flight.
        let c = node(&g, "c");
        assert!(claim(&live, c));

        let mut conn_a = UnixStream::connect(&sock).unwrap();
        conn_a.write_all(b"/nix/store/a\n").unwrap();
        conn_a
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut ack = [0u8; 1];
        assert!(conn_a.read_exact(&mut ack).is_err(), "a acked too early");

        // c is now protected (by a's protect) but still pending. b's
        // protect must not ack yet.
        let mut conn_b = UnixStream::connect(&sock).unwrap();
        conn_b.write_all(b"/nix/store/b\n").unwrap();
        conn_b
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        assert!(conn_b.read_exact(&mut ack).is_err(), "b acked too early");

        // c finishes, both unblock.
        live.end_delete_node(c);
        for conn in [&mut conn_a, &mut conn_b] {
            conn.set_read_timeout(None).unwrap();
            conn.read_exact(&mut ack).unwrap();
            assert_eq!(ack, [b'1']);
        }
    }

    #[test]
    fn claim_nodes_partitions_and_blocks_protect() {
        let g = graph(&[("a", &[]), ("b", &[])]);
        let live = Arc::new(LiveSet::new(g.len()));
        let dir = tempfile::tempdir().unwrap();
        let _server = GcSocketServer::start(dir.path(), Arc::clone(&live), Arc::clone(&g)).unwrap();
        let sock = dir.path().join("gc-socket/socket");

        // Protect b up front. claim must skip it and claim a.
        let mut conn = UnixStream::connect(&sock).unwrap();
        conn.write_all(b"/nix/store/b\n").unwrap();
        let mut ack = [0u8; 1];
        conn.read_exact(&mut ack).unwrap();

        let (a, b) = (node(&g, "a"), node(&g, "b"));
        let (claimed, skipped) = live.claim_nodes(&[a, b]);
        assert_eq!(claimed, vec![a]);
        assert_eq!(skipped, vec![b]);

        // A protect for the claimed node must block until its unlink is
        // done. The DB row is already gone, so acking earlier would tell
        // the builder a deleted row is protected.
        conn.write_all(b"/nix/store/a\n").unwrap();
        conn.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        assert!(conn.read_exact(&mut ack).is_err(), "acked mid-deletion");

        live.end_delete_node(a);
        conn.set_read_timeout(None).unwrap();
        conn.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [b'1']);
    }

    #[test]
    fn drop_joins_handlers_and_snapshot_sees_acked_roots() {
        // Every root acked before the server is dropped must be visible
        // in the snapshot taken right after, even while the client
        // connection is still open.
        let g = graph(&[]);
        let live = Arc::new(LiveSet::new(0));
        let dir = tempfile::tempdir().unwrap();
        let server = GcSocketServer::start(dir.path(), Arc::clone(&live), g).unwrap();
        let sock = dir.path().join("gc-socket/socket");

        let mut conn = UnixStream::connect(&sock).unwrap();
        conn.write_all(b"/nix/store/early-root\n").unwrap();
        let mut ack = [0u8; 1];
        conn.read_exact(&mut ack).unwrap();
        assert_eq!(ack, [b'1']);

        // The handler is blocked in read_line. Drop must not hang.
        let (tx, rx) = mpsc::channel();
        let joiner = std::thread::spawn(move || {
            drop(server);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(2))
            .expect("GcSocketServer::drop hung joining a live connection");
        joiner.join().unwrap();

        assert!(
            live.protected_unknown_snapshot().contains("early-root"),
            "acked root missing from post-drop snapshot"
        );
    }

    #[test]
    fn unknown_path_protected_by_basename() {
        let g = graph(&[]);
        let live = Arc::new(LiveSet::new(0));
        let dir = tempfile::tempdir().unwrap();
        let _server = GcSocketServer::start(dir.path(), Arc::clone(&live), Arc::clone(&g)).unwrap();
        let sock = dir.path().join("gc-socket/socket");

        let mut conn = UnixStream::connect(&sock).unwrap();
        conn.write_all(b"/nix/store/zzz-fresh\n").unwrap();
        let mut ack = [0u8; 1];
        conn.read_exact(&mut ack).unwrap();

        assert!(!live.try_begin_delete_unknown("zzz-fresh"));
        assert!(live.try_begin_delete_unknown("other"));
        live.end_delete_unknown("other");
    }
}

#[cfg(test)]
mod stress_tests;
