// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Garbage collection: liveness computation and store path deletion.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use harmonia_store_db::{BasenameIndex, Closure, GraphOptions, NodeIdx, StoreGraph};
use nix::fcntl::{Flock, FlockArg};
use rayon::prelude::*;
use tracing::{debug, info, warn};

use crate::error::{Error, Result};
use crate::gc_socket::{GcSocketServer, LiveSet};
use crate::roots::find_roots;
use crate::store::GcStore;
use crate::temp_roots::find_temp_roots;

/// Delete a store path from disk, returning bytes freed.
///
/// Store paths are read-only on disk, so directories are made writable
/// before removal, mirroring Nix's `deletePath`.
fn delete_store_path(real_path: &Path) -> Result<u64> {
    let meta = match fs::symlink_metadata(real_path) {
        Ok(m) => m,
        // Already gone: another process won the race.
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
        Err(source) => {
            return Err(Error::Stat {
                path: real_path.to_owned(),
                source,
            });
        }
    };

    if !meta.file_type().is_dir() {
        let bytes = meta.blocks() * 512;
        fs::remove_file(real_path).map_err(|source| Error::Remove {
            path: real_path.to_owned(),
            source,
        })?;
        return Ok(bytes);
    }

    let mut bytes_freed = 0u64;
    // chmod and retry on permission errors; keep going on failure so one
    // bad entry does not leave the rest behind, but report it at the end.
    let mut last_err = None;
    for entry in walkdir::WalkDir::new(real_path).contents_first(true) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                if let Some(parent) = e.path().and_then(Path::parent) {
                    let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o755));
                }
                last_err = Some((
                    e.path().unwrap_or(real_path).to_owned(),
                    e.into_io_error()
                        .unwrap_or_else(|| io::Error::other("walkdir loop")),
                ));
                continue;
            }
        };
        let p = entry.path();
        if let Ok(m) = entry.metadata() {
            bytes_freed += m.blocks() * 512;
        }
        let remove = |p: &Path| {
            if entry.file_type().is_dir() {
                fs::remove_dir(p)
            } else {
                fs::remove_file(p)
            }
        };
        if remove(p).is_err() {
            // Removal needs write permission on the parent, not on p.
            if let Some(parent) = p.parent() {
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o755));
            }
            if let Err(e) = remove(p) {
                last_err = Some((p.to_owned(), e));
            }
        }
    }

    match last_err {
        Some((path, source)) => Err(Error::Remove { path, source }),
        None => Ok(bytes_freed),
    }
}

/// Lock a `tmp-*` build dir before deleting. `None` means a builder
/// still holds it. The caller keeps the fd through deletion to avoid a
/// TOCTOU race.
fn try_lock_dir(path: &Path) -> Option<Flock<fs::File>> {
    // O_NONBLOCK: a stray FIFO named tmp-* must not block open() forever
    // while we hold the exclusive gc.lock.
    let f = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) => {
            if e.kind() != ErrorKind::NotFound {
                warn!("cannot open {} for locking: {e}", path.display());
            }
            return None;
        }
    };
    Flock::lock(f, FlockArg::LockExclusiveNonblock).ok()
}

/// Take the same lock Nix takes. Builders hold it shared while
/// registering temp roots. We take it exclusive so the root set cannot
/// change under us.
fn acquire_gc_lock(state_dir: &Path) -> Result<Flock<fs::File>> {
    let lock_path = state_dir.join("gc.lock");
    let lock_err = |source| Error::GcLock {
        path: lock_path.clone(),
        source,
    };
    // 0600 like Nix: a world-readable lock would let any local user
    // flock it and block GC and builders indefinitely.
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .map_err(lock_err)?;

    match Flock::lock(f, FlockArg::LockExclusiveNonblock) {
        Ok(lock) => Ok(lock),
        Err((f, _)) => {
            info!("waiting for the big garbage collector lock...");
            Flock::lock(f, FlockArg::LockExclusive).map_err(|(_, e)| lock_err(e.into()))
        }
    }
}

/// Tuning knobs for one garbage collection run.
pub struct GcOptions {
    /// Which liveness edges the reference graph should include.
    pub graph_options: GraphOptions,
    /// Report what would be deleted without touching anything.
    pub dry_run: bool,
    /// Stop after freeing this many bytes.
    pub max_freed: Option<u64>,
    /// Keep paths registered at or after this time.
    pub keep_recent_after: Option<SystemTime>,
    /// Run VACUUM after deletion. Disable on busy builders, where
    /// concurrent readers keep the VACUUM's database-sized WAL from
    /// being truncated.
    pub vacuum: bool,
    /// Max dead paths invalidated per database transaction. Smaller keeps
    /// the WAL lower, larger means fewer checkpoints.
    pub chunk_size: usize,
    /// Extra directories scanned for GC roots. Nix only scans its fixed
    /// state dirs, so roots outside them would go uncounted.
    pub extra_gc_roots_dirs: Vec<PathBuf>,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            graph_options: GraphOptions::default(),
            dry_run: false,
            max_freed: None,
            keep_recent_after: None,
            vacuum: true,
            chunk_size: 65_536,
            extra_gc_roots_dirs: Vec::new(),
        }
    }
}

/// Result of a garbage collection run.
///
/// In a dry run `bytes_freed` is the estimated NAR size of `would_delete`
/// and `paths_deleted` is its length.
#[derive(Debug, Clone, Default)]
pub struct GcReport {
    /// Bytes freed on disk, including hard-link savings reclaimed from
    /// `.links`.
    pub bytes_freed: u64,
    /// Number of store paths deleted.
    pub paths_deleted: u64,
    /// Dry run only: full store paths that would be deleted.
    pub would_delete: Vec<String>,
}

/// Collect garbage: find roots, compute the alive closure, delete dead
/// paths.
///
/// Interoperates with running builds throughout: the exclusive `gc.lock`
/// is held, temp roots are honored, and new roots arriving over the
/// gc-socket are protected before their closure is touched.
///
/// The store directory must be writable. On NixOS, where /nix/store is
/// bind-mounted read-only, the caller has to remount it first (see the
/// harmonia-gc binary).
pub fn collect_garbage(store: &GcStore, opts: &GcOptions) -> Result<GcReport> {
    let dry_run = opts.dry_run;
    let max_freed = opts.max_freed;

    // Free the reserved space file first. On a 100% full disk the SQLite
    // invalidation needs room to write before anything was unlinked.
    if !dry_run {
        let reserved = store.layout.state_dir().join("db/reserved");
        match fs::remove_file(&reserved) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => warn!("cannot remove {}: {e}", reserved.display()),
        }
    }

    let _gc_lock = acquire_gc_lock(store.layout.state_dir())?;

    // Serve the gc-socket immediately after taking the lock, like Nix:
    // builders that lost the shared-lock race retry connecting every
    // 100ms, so the socket must exist before the potentially long graph
    // load, and during dry runs, which hold the lock too.
    //
    // Phase 1: no graph yet, so every received root is acked instantly
    // and recorded by basename. That is sound because nothing can be
    // deleted before the graph exists. The roots are replayed below.
    let store_prefix = format!("{}/", store.layout.store_dir().to_str());
    let early_live = Arc::new(LiveSet::new(0));
    // A dry run on a read-only state dir can still report. Without the
    // socket no builder can run anyway (they need temproots).
    let start_socket = |live: Arc<LiveSet>, graph: Arc<StoreGraph>| -> Result<_> {
        match GcSocketServer::start(store.layout.state_dir(), live, graph) {
            Ok(s) => Ok(Some(s)),
            Err(e) if dry_run => {
                warn!("cannot serve gc-socket: {e}");
                Ok(None)
            }
            Err(e) => Err(e),
        }
    };
    let early_socket = start_socket(
        Arc::clone(&early_live),
        Arc::new(StoreGraph::empty(store_prefix.clone())),
    )?;

    // Test sync point: block until the named fifo is readable, so tests
    // can deterministically exercise the early-socket window.
    if let Ok(p) = env::var("_HARMONIA_GC_TEST_SYNC_EARLY") {
        let _ = fs::read(&p);
    }

    info!("loading store graph...");
    let graph = Arc::new(
        store
            .db
            .load_graph(store.layout.store_dir(), &opts.graph_options)?,
    );
    info!("{} total valid paths", graph.len());

    // Phase 2: swap to the real server. Builders whose connection drops
    // during the swap reconnect (Nix's addTempRoot restart loop).
    drop(early_socket);
    let early_roots = early_live.protected_unknown_snapshot();
    let live = Arc::new(LiveSet::new(graph.len()));
    let _gc_socket = start_socket(Arc::clone(&live), Arc::clone(&graph))?;

    let bidx = BasenameIndex::new(&graph);

    // Replay phase-1 roots: known paths become ordinary GC roots (their
    // closure stays alive), unknown basenames stay protected.
    let mut early_root_nodes: Vec<NodeIdx> = Vec::new();
    for b in &early_roots {
        match bidx.get_basename(b) {
            Some(n) => early_root_nodes.push(n),
            None => live.protect_unknown_basename(b),
        }
    }

    // A --store-dir that does not match the DB contents would make every
    // root lookup miss and every DB path look dead, wiping the store.
    if !graph.is_empty() && bidx.is_empty() {
        let first = graph.nodes().next().expect("graph is non-empty");
        return Err(Error::StoreDirMismatch {
            store_dir: store.layout.store_dir().to_path().to_owned(),
            example: graph.path(first).to_owned(),
        });
    }

    info!("finding garbage collector roots...");
    let mut roots = find_roots(&store.layout, &opts.extra_gc_roots_dirs, &bidx)?;
    roots.extend(early_root_nodes);

    // Add temp roots. Some may reference paths registered after our graph
    // snapshot (a builder can register paths while we hold gc.lock as
    // long as it wrote its temp root before we acquired it). Track those
    // by basename so the unknown-on-disk scan won't delete them.
    let mut temp_root_basenames: crate::HashSet<String> = crate::HashSet::default();
    // Nix matches temp roots by hash part so that sibling files of an
    // active build (`<path>.lock`, `<path>.chroot`, `<path>.check`) are
    // protected too.
    let mut temp_root_hashes: crate::HashSet<String> = crate::HashSet::default();
    for tr in find_temp_roots(store.layout.state_dir())? {
        if let Some(b) = tr.strip_prefix(&store_prefix) {
            if b.len() > 32 && b.as_bytes()[32] == b'-' {
                temp_root_hashes.insert(b[..32].to_owned());
            }
            if bidx.get_basename(b).is_none() {
                temp_root_basenames.insert(b.to_owned());
            }
        }
        if let Some(n) = bidx.get(&tr) {
            roots.push(n);
        }
    }
    // --keep-recent: treat recently registered paths as roots.
    if let Some(cutoff) = opts.keep_recent_after {
        let cutoff = cutoff
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        let n_before = roots.len();
        roots.extend(
            graph
                .nodes()
                .filter(|&n| graph.registration_time(n) >= cutoff),
        );
        info!("{} recent paths kept", roots.len() - n_before);
    }

    roots.sort_unstable();
    roots.dedup();
    info!("found {} roots", roots.len());

    info!("computing alive closure...");
    let alive = graph.compute_closure(&roots);
    let n_alive = alive.len();
    info!("{} alive paths", n_alive);
    info!("{} dead paths", graph.len() - n_alive);

    // Find entries on disk that are not in the DB at all. Raw OsString
    // names: a non-UTF-8 entry cannot be in the DB (it stores text), but
    // it is still garbage that must be unlinked by its real bytes.
    let mut unknown_on_disk: Vec<OsString> = Vec::new();
    match fs::read_dir(store.layout.real_store_dir()) {
        Err(e) => warn!(
            "cannot list {}: {e}",
            store.layout.real_store_dir().display()
        ),
        Ok(entries) => {
            for entry in entries.flatten() {
                let raw = entry.file_name();
                if let Some(name) = raw.to_str() {
                    if name == ".links" {
                        continue;
                    }
                    // An entry belongs to an active build if its first 32
                    // chars match a temp root's hash part.
                    let hash_part_active = name.len() >= 32
                        && name.is_char_boundary(32)
                        && temp_root_hashes.contains(&name[..32]);
                    if bidx.get_basename(name).is_some()
                        || temp_root_basenames.contains(name)
                        || hash_part_active
                    {
                        continue;
                    }
                }
                unknown_on_disk.push(raw);
            }
        }
    }
    if !unknown_on_disk.is_empty() {
        info!("{} unknown paths on disk not in DB", unknown_on_disk.len());
    }

    let dead_nodes: Vec<NodeIdx> = graph.nodes().filter(|&n| !alive.contains(n)).collect();

    let max = max_freed.unwrap_or(u64::MAX);

    if dry_run {
        let mut estimated = 0u64;
        let mut would_delete = Vec::new();
        // Roots can arrive over the gc-socket while we run. A real GC
        // would honor them, so the report must too.
        let protected = live.protected_snapshot();
        let protected_unknown = live.protected_unknown_snapshot();
        for &node in &dead_nodes {
            if estimated >= max {
                break;
            }
            if protected[node.index()] {
                continue;
            }
            would_delete.push(graph.path(node).to_owned());
            estimated += graph.nar_size(node);
        }
        if estimated < max {
            for name in &unknown_on_disk {
                if name.to_str().is_some_and(|n| protected_unknown.contains(n)) {
                    continue;
                }
                would_delete.push(format!("{store_prefix}{}", name.display()));
            }
        }
        return Ok(GcReport {
            bytes_freed: estimated,
            paths_deleted: would_delete.len() as u64,
            would_delete,
        });
    }

    info!("deleting garbage...");

    // Test sync point, so tests exercise the protect() path
    // deterministically rather than racing the delete loop.
    if let Ok(p) = env::var("_HARMONIA_GC_TEST_SYNC") {
        let _ = fs::read(&p);
    }

    // Bulk-invalidate, then delete from disk in parallel. Safe to crash
    // mid-delete: leftover dirs are picked up as unknown-on-disk next run.
    let real_store_dir = store.layout.real_store_dir().to_path_buf();
    let bytes_freed = AtomicU64::new(0);
    let paths_deleted = AtomicU64::new(0);

    let (order, acyclic_len) = deletion_order(&graph, &alive, &dead_nodes);

    // Commit in bounded chunks, truncating the WAL after each, so disk
    // use stays bounded and space is reclaimed incrementally.
    let max_chunk = opts.chunk_size.max(1);
    let mut cursor = 0usize;
    while cursor < order.len() {
        let freed_so_far = bytes_freed.load(Ordering::Relaxed);
        if freed_so_far >= max {
            info!("deleted more than {max} bytes, stopping");
            break;
        }
        let remaining = max - freed_so_far;
        let mut chunk: Vec<NodeIdx> = Vec::new();
        // narSize over-reports hard-linked paths, so estimated only
        // bounds the chunk. Actual freed bytes are re-checked above.
        let mut estimated = 0u64;
        // Take the cyclic tail (from acyclic_len on) whole and unsplit.
        let take_all = cursor >= acyclic_len;
        while cursor < order.len()
            && (take_all
                || (cursor < acyclic_len && estimated < remaining && chunk.len() < max_chunk))
        {
            let node = order[cursor];
            cursor += 1;
            estimated = estimated.saturating_add(graph.nar_size(node));
            chunk.push(node);
        }
        if chunk.is_empty() {
            break;
        }
        // Claim atomically with the protection check, *before*
        // invalidating DB rows. A protect() arriving for a claimed node
        // blocks until the unlink finished, so a builder is never acked
        // while the row deletion is in flight.
        let (mut claimed, skipped) = live.claim_nodes(&chunk);
        // protect() marks closures atomically, but the claim is per node:
        // it may have kept a reference whose referrer got protected a
        // moment earlier. Drop the closures of skipped paths.
        if !skipped.is_empty() {
            let keep_out = graph.compute_closure(&skipped);
            claimed.retain(|&n| {
                if keep_out.contains(n) {
                    live.end_delete_node(n);
                    false
                } else {
                    true
                }
            });
        }
        // Invalidate rows before unlinking: builders trust isValidPath(),
        // so a path must never look valid after its disk entry is gone.
        store
            .db
            .invalidate_ids(claimed.iter().map(|&n| graph.db_id(n)))?;
        claimed.par_iter().for_each(|&node| {
            let path = graph.path(node);
            let basename = path.strip_prefix(&store_prefix).unwrap_or(path);
            let real_path = real_store_dir.join(basename);
            debug!("deleting '{path}'");
            match delete_store_path(&real_path) {
                Ok(freed) => {
                    bytes_freed.fetch_add(freed, Ordering::Relaxed);
                    paths_deleted.fetch_add(1, Ordering::Relaxed);
                }
                // The row is already invalidated. The leftover is picked
                // up as unknown-on-disk by the next run.
                Err(e) => warn!("failed to delete {}: {e}", real_path.display()),
            }
            live.end_delete_node(node);
        });
        debug!(
            "deleted {}/{} dead paths, {} bytes freed",
            paths_deleted.load(Ordering::Relaxed),
            order.len(),
            bytes_freed.load(Ordering::Relaxed),
        );
    }

    // Unknown-on-disk paths, also in parallel. tmp-* dirs hold flock
    // through deletion to avoid a TOCTOU race with a builder.
    if bytes_freed.load(Ordering::Relaxed) < max {
        // A builder may have registered a scanned path after our graph
        // snapshot and then exited, leaving its temp root file stale.
        // Unlinking such a path would orphan its ValidPaths row, so
        // re-check the DB first. Once is enough: any registration from
        // here on goes through the gc-socket and protected_unknown.
        let mut unknown_on_disk = unknown_on_disk;
        unknown_on_disk.retain(|name| {
            // A non-UTF-8 name cannot be a DB path (the DB stores text).
            let Some(name) = name.to_str() else {
                return true;
            };
            match is_valid_path(store, &format!("{store_prefix}{name}")) {
                Ok(valid) => !valid,
                Err(e) => {
                    warn!("skipping {store_prefix}{name}: validity check failed: {e}");
                    false
                }
            }
        });
        unknown_on_disk.par_iter().for_each(|raw| {
            // The liveset key is textual. Non-UTF-8 names cannot collide
            // with anything a builder protects.
            let name = raw.to_string_lossy();
            if !live.try_begin_delete_unknown(&name) {
                return;
            }
            let real_path = real_store_dir.join(raw);
            // Only a directory can be a build temp dir a builder still
            // holds. A stray tmp-* FIFO would fail flock() on macOS and
            // be kept forever.
            let is_locked_candidate = name.starts_with("tmp-")
                && real_path
                    .symlink_metadata()
                    .map(|m| m.is_dir())
                    .unwrap_or(false);
            let _tmp_lock = if is_locked_candidate {
                match try_lock_dir(&real_path) {
                    Some(f) => Some(f),
                    None => {
                        debug!("skipping locked tempdir {}", real_path.display());
                        live.end_delete_unknown(&name);
                        return;
                    }
                }
            } else {
                None
            };
            debug!("deleting '{store_prefix}{name}'");
            match delete_store_path(&real_path) {
                Ok(freed) => {
                    bytes_freed.fetch_add(freed, Ordering::Relaxed);
                    paths_deleted.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => warn!("failed to delete {}: {e}", real_path.display()),
            }
            live.end_delete_unknown(&name);
        });
    }

    let bytes_freed = bytes_freed.into_inner();
    let paths_deleted = paths_deleted.into_inner();

    let bytes_freed = bytes_freed + clean_links(store.layout.links_dir())?;

    // Reclaim db space freed by the row deletions, still under the
    // exclusive gc.lock. Best effort: a failed vacuum leaves the db
    // valid.
    if opts.vacuum
        && let Err(e) = store.db.maybe_vacuum()
    {
        warn!("vacuuming database failed: {e}");
    }

    Ok(GcReport {
        bytes_freed,
        paths_deleted,
        would_delete: Vec::new(),
    })
}

/// Check a raw path string against ValidPaths. Unknown-on-disk names are
/// arbitrary bytes, so this cannot go through the typed StorePath API.
/// Referrer-first deletion order (Kahn over the dead subgraph), so every
/// prefix is safe to commit: still-valid paths never reference an
/// already-deleted one. Returns the order and the length of its acyclic
/// prefix; cyclic nodes never reach in-degree 0 and are appended as one
/// trailing group that must not be split.
fn deletion_order(
    graph: &StoreGraph,
    alive: &Closure,
    dead_nodes: &[NodeIdx],
) -> (Vec<NodeIdx>, usize) {
    // in_degree counts a dead node's dead referrers.
    let dead_ref = |node: NodeIdx, m: NodeIdx| m != node && !alive.contains(m);
    let mut in_degree = vec![0u32; graph.len()];
    for &node in dead_nodes {
        for m in graph.refs(node).filter(|&m| dead_ref(node, m)) {
            in_degree[m.index()] += 1;
        }
    }
    let mut order: Vec<NodeIdx> = dead_nodes
        .iter()
        .copied()
        .filter(|&m| in_degree[m.index()] == 0)
        .collect();
    let mut head = 0;
    while head < order.len() {
        let node = order[head];
        head += 1;
        for m in graph.refs(node).filter(|&m| dead_ref(node, m)) {
            in_degree[m.index()] -= 1;
            if in_degree[m.index()] == 0 {
                order.push(m);
            }
        }
    }
    let acyclic_len = order.len();
    if order.len() < dead_nodes.len() {
        order.extend(
            dead_nodes
                .iter()
                .copied()
                .filter(|&m| in_degree[m.index()] != 0),
        );
    }
    (order, acyclic_len)
}

fn is_valid_path(store: &GcStore, path: &str) -> Result<bool> {
    // Cached: called once per unknown-on-disk entry, which can be many
    // thousands after an interrupted nixos-install or nix copy.
    let mut stmt = store
        .db
        .connection()
        .prepare_cached("SELECT COUNT(*) FROM ValidPaths WHERE path = ?")
        .map_err(harmonia_store_db::Error::from)?;
    let n: i64 = stmt
        .query_row([path], |r| r.get(0))
        .map_err(harmonia_store_db::Error::from)?;
    Ok(n > 0)
}

/// Remove hard links with link count 1 from the `.links` directory,
/// returning bytes freed.
///
/// The dir can contain millions of entries. stat + unlink per entry is
/// disk-bound, so process in parallel, and stream the entries instead of
/// collecting them: a Vec of millions of DirEntries costs gigabytes.
fn clean_links(links_dir: &Path) -> Result<u64> {
    info!("deleting unused links...");
    let entries = match fs::read_dir(links_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            warn!("cannot read {}: {e}", links_dir.display());
            return Ok(0);
        }
    };

    // A link entry has one reference from .links plus one per store file.
    // N references total mean hard linking saves (N-2)*size compared to
    // independent copies.
    let saved_bytes = AtomicU64::new(0);
    let freed_bytes = AtomicU64::new(0);

    entries.flatten().par_bridge().for_each(|entry| {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            return;
        };
        if meta.nlink() != 1 {
            saved_bytes.fetch_add(
                meta.nlink().saturating_sub(2) * meta.size(),
                Ordering::Relaxed,
            );
            return;
        }
        if fs::remove_file(&path).is_ok() {
            freed_bytes.fetch_add(meta.blocks() * 512, Ordering::Relaxed);
        }
    });

    let saving = saved_bytes.into_inner();
    if saving > 0 {
        info!("hard linking is currently saving {saving} bytes");
    }

    Ok(freed_bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::graph;

    #[test]
    fn deletion_order_referrers_first_cycle_last() {
        // app -> lib -> libc(alive);  a <-> b cycle;  leaf standalone.
        let g = graph(&[
            ("app", &[1]),
            ("lib", &[2]),
            ("libc", &[]),
            ("a", &[4]),
            ("b", &[3]),
            ("leaf", &[]),
        ]);
        let n = |name: &str| {
            g.nodes()
                .find(|&n| g.path(n) == format!("/nix/store/{name}"))
                .unwrap()
        };
        let alive = g.compute_closure(&[n("libc")]);
        let dead: Vec<_> = g.nodes().filter(|&x| !alive.contains(x)).collect();

        let (order, acyclic_len) = deletion_order(&g, &alive, &dead);

        assert_eq!(order.len(), dead.len());
        assert!(!order.contains(&n("libc")));
        let pos = |x| order.iter().position(|&o| o == x).unwrap();
        assert!(pos(n("app")) < pos(n("lib")), "referrer before reference");
        assert_eq!(acyclic_len, 3, "app, lib, leaf are acyclic");
        let mut tail: Vec<_> = order[acyclic_len..].to_vec();
        tail.sort();
        let mut cyc = vec![n("a"), n("b")];
        cyc.sort();
        assert_eq!(tail, cyc, "cycle forms the unsplit tail");
    }

    #[test]
    fn delete_store_path_missing_is_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(delete_store_path(&tmp.path().join("gone")).unwrap(), 0);
    }

    #[test]
    fn delete_store_path_other_stat_errors_propagate() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("file");
        fs::write(&f, b"x").unwrap();
        // ENOTDIR, not ENOENT: must not be treated as "already gone".
        assert!(delete_store_path(&f.join("sub")).is_err());
    }

    #[test]
    fn delete_store_path_file_reports_disk_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("file");
        fs::write(&f, vec![1u8; 5000]).unwrap();
        let expected = fs::symlink_metadata(&f).unwrap().blocks() * 512;
        assert!(expected > 0);
        assert_eq!(delete_store_path(&f).unwrap(), expected);
        assert!(!f.exists());
    }

    #[test]
    fn delete_store_path_removes_readonly_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("pkg");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/file"), vec![1u8; 5000]).unwrap();
        let mut expected = 0;
        for e in walkdir::WalkDir::new(&dir) {
            expected += e.unwrap().metadata().unwrap().blocks() * 512;
        }
        // Store paths are read-only on disk.
        fs::set_permissions(dir.join("sub/file"), fs::Permissions::from_mode(0o444)).unwrap();
        fs::set_permissions(dir.join("sub"), fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();

        assert_eq!(delete_store_path(&dir).unwrap(), expected);
        assert!(!dir.exists());
    }

    #[test]
    fn try_lock_dir_none_while_held() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = try_lock_dir(tmp.path()).expect("unheld dir is lockable");
        assert!(try_lock_dir(tmp.path()).is_none(), "held dir must not lock");
        drop(lock);
    }

    #[test]
    fn clean_links_removes_only_unreferenced() {
        let tmp = tempfile::tempdir().unwrap();
        let links = tmp.path().join(".links");
        fs::create_dir_all(&links).unwrap();
        let dead = links.join("dead");
        fs::write(&dead, b"unreferenced").unwrap();
        let shared = links.join("shared");
        fs::write(&shared, b"referenced").unwrap();
        fs::hard_link(&shared, tmp.path().join("user")).unwrap();

        let dead_blocks = fs::symlink_metadata(&dead).unwrap().blocks() * 512;
        let freed = clean_links(&links).unwrap();

        assert!(!dead.exists());
        assert!(shared.exists());
        assert_eq!(freed, dead_blocks, "freed bytes of removed links");
        // A missing .links dir is not an error.
        assert_eq!(clean_links(&tmp.path().join("nope")).unwrap(), 0);
    }
}
