// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Store handle for garbage collection.

use std::fs;
use std::path::{Path, PathBuf};

use harmonia_store_db::{GraphOptions, OpenMode, StoreDb};
use harmonia_store_path::StoreDir;

use crate::error::Result;

/// Everything the garbage collector needs to know about one store: the
/// open database plus the directory layout around it.
///
/// The GC settings in [`GcStore::graph_options`] are initialized from the
/// host's nix.conf (`keep-derivations`, `keep-outputs`) and can be
/// overridden before calling [`crate::gc::collect_garbage`].
///
/// ```no_run
/// # fn main() -> harmonia_store_gc::Result<()> {
/// use std::path::Path;
/// use harmonia_store_gc::store::GcStore;
///
/// let store = GcStore::open(Path::new("/nix/store"), Path::new("/nix/var/nix"))?;
/// # Ok(())
/// # }
/// ```
pub struct GcStore {
    /// The store database at `state_dir/db/db.sqlite`.
    pub db: StoreDb,
    /// The logical store directory, e.g. `/nix/store`. This is the form
    /// recorded in the database.
    pub store_dir: StoreDir,
    /// The Nix state directory, e.g. `/nix/var/nix`. Holds `gc.lock`,
    /// `temproots/`, `gc-socket/` and the database.
    pub state_dir: PathBuf,
    /// The physical store directory: `store_dir` with symlinks resolved.
    /// The database records logical paths, but disk operations need real
    /// ones.
    pub real_store_dir: PathBuf,
    /// The hard-link dedup directory `real_store_dir/.links`.
    pub links_dir: PathBuf,
    /// Which liveness edges the reference graph should include.
    pub graph_options: GraphOptions,
}

impl GcStore {
    /// Open the store database read-write, as needed for an actual
    /// collection.
    pub fn open(store_dir: &Path, state_dir: &Path) -> Result<Self> {
        Self::open_with_mode(store_dir, state_dir, false)
    }

    /// Open the store database read-only, for dry runs. No journal-mode
    /// flip, no write lock. This also works on a read-only filesystem.
    pub fn open_read_only(store_dir: &Path, state_dir: &Path) -> Result<Self> {
        Self::open_with_mode(store_dir, state_dir, true)
    }

    fn open_with_mode(store_dir: &Path, state_dir: &Path, read_only: bool) -> Result<Self> {
        let store_dir = StoreDir::new(normalize_dir(store_dir))?;
        let db_path = state_dir.join("db/db.sqlite");
        let db = if read_only {
            StoreDb::open_readonly(&db_path)?
        } else {
            let db = StoreDb::open(&db_path, OpenMode::ReadWrite)?;
            configure_for_gc(&db)?;
            db
        };

        // Fall back to the logical dir when it cannot be resolved, like
        // Nix's realStoreDir.
        let real_store_dir =
            fs::canonicalize(store_dir.to_path()).unwrap_or_else(|_| store_dir.to_path().into());

        Ok(GcStore {
            db,
            state_dir: state_dir.to_owned(),
            links_dir: real_store_dir.join(".links"),
            real_store_dir,
            store_dir,
            graph_options: GraphOptions {
                keep_derivations: crate::config::bool_setting("keep-derivations", true),
                keep_outputs: crate::config::bool_setting("keep-outputs", false),
            },
        })
    }
}

/// Prepare the connection for GC writes.
fn configure_for_gc(db: &StoreDb) -> Result<()> {
    let conn = db.connection();
    // Install the busy handler first: the journal-mode flip takes a write
    // lock and would otherwise fail instantly while the daemon writes.
    conn.busy_timeout(std::time::Duration::from_secs(60))
        .map_err(harmonia_store_db::Error::from)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(harmonia_store_db::Error::from)?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(harmonia_store_db::Error::from)?;
    Ok(())
}

/// Strip trailing slashes but keep a bare "/".
///
/// The store prefix comparisons throughout the GC append their own '/'.
/// With a trailing slash left in, every prefix check would miss and the
/// whole store would look dead.
fn normalize_dir(dir: &Path) -> PathBuf {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let bytes = dir.as_os_str().as_bytes();
    let mut end = bytes.len();
    while end > 1 && bytes[end - 1] == b'/' {
        end -= 1;
    }
    PathBuf::from(std::ffi::OsString::from_vec(bytes[..end].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_dir_strips_trailing_slashes() {
        assert_eq!(
            normalize_dir(Path::new("/nix/store/")),
            Path::new("/nix/store")
        );
        assert_eq!(
            normalize_dir(Path::new("/nix/store//")),
            Path::new("/nix/store")
        );
        assert_eq!(
            normalize_dir(Path::new("/nix/store")),
            Path::new("/nix/store")
        );
        assert_eq!(normalize_dir(Path::new("/")), Path::new("/"));
    }
}
