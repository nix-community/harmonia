// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! In-memory snapshot of the store reference graph.
//!
//! Walking the reference graph with one SQLite query per path is slow: a
//! store with 100K paths means 100K B-tree seeks. [`StoreDb::load_graph`]
//! instead reads `ValidPaths` and `Refs` once into a compressed sparse row
//! (CSR) adjacency list, so liveness computation is a BFS over integer node
//! ids ([`StoreGraph::compute_closure`]).

use std::collections::HashMap;
use std::time::Instant;

use foldhash::{HashMap as IdMap, HashMapExt};

use harmonia_store_path::StoreDir;
use rusqlite::Connection;
use tracing::debug;

use crate::connection::StoreDb;
use crate::error::{Error, Result};

/// Which extra GC liveness edges to include in the graph.
///
/// The fields mirror the `keep-derivations` and `keep-outputs` settings in
/// nix.conf and default to Nix's defaults. Reading nix.conf is the caller's
/// responsibility. The generated edges match exactly what Nix's own GC
/// propagates.
#[derive(Debug, Clone, Copy)]
pub struct GraphOptions {
    /// An alive path keeps its deriver alive.
    ///
    /// Adds output → drv edges, from `ValidPaths.deriver` *only*. A
    /// `DerivationOutputs` or `BuildTraceV3` row mapping one of a drv's
    /// outputs does not pin the drv. Nix considers a drv garbage once the
    /// output's recorded deriver is a different (newer) drv.
    pub keep_derivations: bool,

    /// An alive `.drv` file keeps its outputs alive.
    ///
    /// Adds drv → output edges, from three sources:
    /// - `DerivationOutputs` (input-addressed outputs)
    /// - `BuildTraceV3` (content-addressed outputs)
    /// - `ValidPaths.deriver`
    ///
    /// Content-addressed outputs are only supported for Nix >= 2.35, which
    /// records them in `BuildTraceV3` under the derivation's store path.
    /// Older Nix used a `Realisations` table that identifies the derivation
    /// by a content hash instead, so its rows cannot be matched back to
    /// store paths. On such stores only the deriver edge pins CA outputs.
    pub keep_outputs: bool,
}

impl Default for GraphOptions {
    fn default() -> Self {
        GraphOptions {
            keep_derivations: true,
            keep_outputs: false,
        }
    }
}

/// Index of a node in a [`StoreGraph`].
///
/// Node indices are dense in `0..graph.len()` and are only valid for the
/// graph that produced them. The `ValidPaths.id` database rowid of a node
/// is available via [`StoreGraph::db_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeIdx(u32);

impl NodeIdx {
    /// Position in `0..graph.len()`, for indexing caller-side per-node data.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Set of alive nodes, produced by [`StoreGraph::compute_closure`].
pub struct Closure(Vec<bool>);

impl Closure {
    pub fn contains(&self, node: NodeIdx) -> bool {
        self.0[node.index()]
    }

    /// Number of alive nodes.
    pub fn len(&self) -> usize {
        self.0.iter().filter(|&&alive| alive).count()
    }

    pub fn is_empty(&self) -> bool {
        !self.0.iter().any(|&alive| alive)
    }
}

/// Snapshot of the store reference graph in CSR layout.
///
/// Nodes are valid store paths. Edges point from referrer to reference.
/// Obtain node handles via [`StoreGraph::nodes`] or [`BasenameIndex`], then
/// query per-node data with the accessor methods.
///
/// ```
/// # fn main() -> harmonia_store_db::Result<()> {
/// use harmonia_store_db::{GraphOptions, StoreDb};
/// use harmonia_store_path::StoreDir;
///
/// let db = StoreDb::open_memory()?;
/// let graph = db.load_graph(&StoreDir::default(), &GraphOptions::default())?;
///
/// let closure = graph.compute_closure(&[]);
/// let dead_bytes: u64 = graph
///     .nodes()
///     .filter(|&n| !closure.contains(n))
///     .map(|n| graph.nar_size(n))
///     .sum();
/// assert_eq!(dead_bytes, 0);
/// # Ok(())
/// # }
/// ```
pub struct StoreGraph {
    paths: Vec<String>,
    ids: Vec<i64>,
    nar_sizes: Vec<u64>,
    registration_times: Vec<i64>,
    /// CSR row offsets: refs of node `i` are
    /// `ref_targets[ref_offsets[i]..ref_offsets[i + 1]]`.
    ref_offsets: Vec<u32>,
    ref_targets: Vec<u32>,
    store_prefix: String,
}

impl StoreGraph {
    /// Graph with no nodes, usable as a placeholder while the real graph is
    /// still loading.
    pub fn empty(store_prefix: String) -> StoreGraph {
        StoreGraph {
            paths: Vec::new(),
            ids: Vec::new(),
            nar_sizes: Vec::new(),
            registration_times: Vec::new(),
            ref_offsets: vec![0],
            ref_targets: Vec::new(),
            store_prefix,
        }
    }

    /// Number of nodes (valid store paths).
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// All nodes, in unspecified order.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = NodeIdx> + use<> {
        (0..self.paths.len() as u32).map(NodeIdx)
    }

    /// Full store path of `node`, e.g. `/nix/store/<hash>-<name>`.
    pub fn path(&self, node: NodeIdx) -> &str {
        &self.paths[node.index()]
    }

    /// `ValidPaths.id` database rowid of `node`, as accepted by
    /// [`StoreDb::invalidate_ids`](crate::StoreDb::invalidate_ids).
    pub fn db_id(&self, node: NodeIdx) -> i64 {
        self.ids[node.index()]
    }

    /// NAR size of `node` in bytes.
    pub fn nar_size(&self, node: NodeIdx) -> u64 {
        self.nar_sizes[node.index()]
    }

    /// Registration time of `node` (Unix epoch seconds).
    pub fn registration_time(&self, node: NodeIdx) -> i64 {
        self.registration_times[node.index()]
    }

    /// Store dir prefix including trailing slash, e.g. `"/nix/store/"`.
    pub fn store_prefix(&self) -> &str {
        &self.store_prefix
    }

    /// References of `node`.
    #[inline]
    pub fn refs(&self, node: NodeIdx) -> impl ExactSizeIterator<Item = NodeIdx> + '_ {
        let start = self.ref_offsets[node.index()] as usize;
        let end = self.ref_offsets[node.index() + 1] as usize;
        self.ref_targets[start..end].iter().map(|&t| NodeIdx(t))
    }

    /// Mark all nodes reachable from `roots`.
    ///
    /// Cycles and duplicate roots are handled.
    pub fn compute_closure(&self, roots: &[NodeIdx]) -> Closure {
        let mut alive = vec![false; self.len()];
        let mut stack: Vec<NodeIdx> = Vec::with_capacity(roots.len());
        for &r in roots {
            if !alive[r.index()] {
                alive[r.index()] = true;
                stack.push(r);
            }
        }
        while let Some(node) = stack.pop() {
            for next in self.refs(node) {
                if !alive[next.index()] {
                    alive[next.index()] = true;
                    stack.push(next);
                }
            }
        }
        Closure(alive)
    }
}

/// Basename -> node lookup for a [`StoreGraph`].
///
/// Built once and borrowed from the graph. Used for resolving GC roots and
/// on-disk directory entries to graph nodes.
///
/// ```
/// # fn main() -> harmonia_store_db::Result<()> {
/// use harmonia_store_db::{BasenameIndex, GraphOptions, StoreDb};
/// use harmonia_store_path::StoreDir;
///
/// let db = StoreDb::open_memory()?;
/// let graph = db.load_graph(&StoreDir::default(), &GraphOptions::default())?;
/// let index = BasenameIndex::new(&graph);
/// assert_eq!(index.get("/nix/store/nonexistent"), None);
/// # Ok(())
/// # }
/// ```
pub struct BasenameIndex<'g> {
    map: HashMap<&'g str, NodeIdx>,
    store_prefix: &'g str,
}

impl<'g> BasenameIndex<'g> {
    pub fn new(graph: &'g StoreGraph) -> Self {
        let mut map: HashMap<&'g str, NodeIdx> = HashMap::with_capacity(graph.paths.len());
        for node in graph.nodes() {
            if let Some(b) = graph.path(node).strip_prefix(&graph.store_prefix) {
                map.insert(b, node);
            }
        }
        BasenameIndex {
            map,
            store_prefix: &graph.store_prefix,
        }
    }

    /// True if no graph path matched the store prefix. On a non-empty
    /// graph this means the configured store dir does not correspond to
    /// the database contents.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Look up a full store path, e.g. `/nix/store/<hash>-<name>`.
    pub fn get(&self, path: &str) -> Option<NodeIdx> {
        let b = path.strip_prefix(self.store_prefix)?;
        self.map.get(b).copied()
    }

    /// Look up a store path basename, e.g. `<hash>-<name>`.
    pub fn get_basename(&self, basename: &str) -> Option<NodeIdx> {
        self.map.get(basename).copied()
    }
}

impl StoreDb {
    /// Load the full reference graph into memory.
    ///
    /// Both node and edge queries run in one transaction so they see the
    /// same snapshot. A path registered between two separate transactions
    /// would otherwise end up with missing edges.
    ///
    /// The snapshot goes stale as soon as other processes register paths.
    /// A garbage collector using it to decide liveness must hold the GC
    /// lock and serve the temp-roots socket for the graph to stay
    /// authoritative.
    ///
    /// See [`StoreGraph`] for an example.
    pub fn load_graph(&self, store_dir: &StoreDir, options: &GraphOptions) -> Result<StoreGraph> {
        self.conn.execute_batch("BEGIN")?;
        let result = load_graph_in_txn(&self.conn, store_dir, options);
        if result.is_err() {
            self.conn.execute_batch("ROLLBACK").ok();
        } else {
            self.conn.execute_batch("COMMIT")?;
        }
        result
    }
}

fn load_graph_in_txn(
    conn: &Connection,
    store_dir: &StoreDir,
    options: &GraphOptions,
) -> Result<StoreGraph> {
    // ValidPaths ids are sparse: every collected path leaves a gap.
    let mut id_to_idx: IdMap<i64, u32> = IdMap::new();

    let mut paths: Vec<String> = Vec::new();
    let mut ids: Vec<i64> = Vec::new();
    let mut nar_sizes: Vec<u64> = Vec::new();
    let mut registration_times: Vec<i64> = Vec::new();
    let t_nodes = Instant::now();
    {
        let mut stmt =
            conn.prepare("SELECT id, path, narSize, registrationTime FROM ValidPaths")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let nar: Option<i64> = row.get(2)?;
            let reg_time: i64 = row.get(3)?;
            id_to_idx.insert(id, paths.len() as u32);
            paths.push(path);
            ids.push(id);
            nar_sizes.push(nar.unwrap_or(0).max(0) as u64);
            registration_times.push(reg_time);
        }
    }
    debug!(
        "loaded {} nodes in {:.1}s",
        paths.len(),
        t_nodes.elapsed().as_secs_f64()
    );

    let n = paths.len();
    let mut edges: Vec<(u32, u32)> = Vec::new();
    let t_edges = Instant::now();

    let mut add_edges = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> Result<()> {
        let start = Instant::now();
        let before = edges.len();
        let mut stmt = conn.prepare(sql)?;
        let mut rows = stmt.query(params)?;
        while let Some(row) = rows.next()? {
            let from_id: i64 = row.get(0)?;
            let to_id: i64 = row.get(1)?;
            // Edges pointing at unknown ids (corrupt DB) are dropped.
            if let (Some(&from), Some(&to)) = (id_to_idx.get(&from_id), id_to_idx.get(&to_id)) {
                edges.push((from, to));
            }
        }
        debug!(
            "edge query took {:.1}s ({} edges): {sql}",
            start.elapsed().as_secs_f64(),
            edges.len() - before,
        );
        Ok(())
    };

    add_edges("SELECT referrer, reference FROM Refs", &[])?;

    // Edge semantics for the keep-* queries are documented on GraphOptions.
    let store_prefix = format!("{}/", store_dir.to_str().trim_end_matches('/'));

    if options.keep_derivations {
        // output → drv
        add_edges(
            "SELECT v.id, d.id FROM ValidPaths v \
             JOIN ValidPaths d ON d.path = v.deriver \
             WHERE v.deriver IS NOT NULL",
            &[],
        )?;
    }

    if options.keep_outputs {
        // drv → output
        add_edges(
            "SELECT do2.drv, o.id FROM DerivationOutputs do2 \
             JOIN ValidPaths o ON o.path = do2.path",
            &[],
        )?;
        // drv → output via the deriver field (also pins CA outputs on
        // stores written by Nix < 2.35, which have no BuildTraceV3)
        add_edges(
            "SELECT d.id, v.id FROM ValidPaths v \
             JOIN ValidPaths d ON d.path = v.deriver \
             WHERE v.deriver IS NOT NULL",
            &[],
        )?;
        // BuildTraceV3 is created lazily. Its drvPath column holds a store
        // path basename.
        if has_table(conn, "BuildTraceV3")? {
            // drv → CA output
            add_edges(
                "SELECT d.id, o.id FROM BuildTraceV3 bt \
                 JOIN ValidPaths d ON d.path = ? || bt.drvPath \
                 JOIN ValidPaths o ON o.path = bt.outputPath",
                &[&store_prefix],
            )?;
        }
    }

    debug!(
        "loaded {} edges in {:.1}s",
        edges.len(),
        t_edges.elapsed().as_secs_f64()
    );

    // The keep-outputs queries can emit the same edge twice.
    edges.sort_unstable();
    edges.dedup();

    // The CSR uses u32 offsets. A graph beyond that is refused, not wrapped.
    let _: u32 = edges.len().try_into().map_err(|_| Error::TooManyEdges)?;

    // Counting sort into CSR: count refs per node, prefix-sum into offsets,
    // then scatter the targets.
    let mut ref_offsets = vec![0u32; n + 1];
    for &(from, _) in &edges {
        ref_offsets[from as usize + 1] += 1;
    }
    for i in 0..n {
        ref_offsets[i + 1] += ref_offsets[i];
    }
    let mut ref_targets = vec![0u32; edges.len()];
    let mut cursor = ref_offsets.clone();
    for &(from, to) in &edges {
        let pos = cursor[from as usize];
        ref_targets[pos as usize] = to;
        cursor[from as usize] += 1;
    }

    Ok(StoreGraph {
        paths,
        ids,
        nar_sizes,
        registration_times,
        ref_offsets,
        ref_targets,
        store_prefix,
    })
}

pub(crate) fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?",
        [name],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}
