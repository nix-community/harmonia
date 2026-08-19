use std::sync::Arc;

use harmonia_store_db::{GraphOptions, StoreDb, StoreGraph};
use harmonia_store_path::StoreDir;

/// Load a graph with the given nodes and edges. `refs` are indices
/// into `paths`.
pub(crate) fn graph(paths: &[(&str, &[usize])]) -> Arc<StoreGraph> {
    let db = StoreDb::open_memory().unwrap();
    let mut ids = Vec::new();
    for (name, _) in paths {
        db.connection()
            .execute(
                "INSERT INTO ValidPaths (path, hash, registrationTime) \
                 VALUES (?, 'sha256:x', 1)",
                [format!("/nix/store/{name}")],
            )
            .unwrap();
        ids.push(db.connection().last_insert_rowid());
    }
    for (i, (_, refs)) in paths.iter().enumerate() {
        for &r in refs.iter() {
            db.connection()
                .execute(
                    "INSERT INTO Refs (referrer, reference) VALUES (?, ?)",
                    [ids[i], ids[r]],
                )
                .unwrap();
        }
    }
    let options = GraphOptions {
        keep_derivations: false,
        keep_outputs: false,
    };
    Arc::new(db.load_graph(&StoreDir::default(), &options).unwrap())
}
