// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Store handle for garbage collection.

use std::path::Path;

use harmonia_store_db::{OpenMode, StoreDb};
use harmonia_store_fs::StoreLayout;

use crate::error::Result;

/// A store database opened for garbage collection, plus its on-disk
/// layout.
///
/// The constructors guarantee the connection is configured for GC use:
/// [`GcStore::open`] installs a busy handler and switches to WAL before
/// any write, [`GcStore::open_read_only`] takes no write lock at all.
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
    /// Where the store lives on disk.
    pub layout: StoreLayout,
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
        let layout = StoreLayout::new(store_dir, state_dir)?;
        let db_path = layout.db_path();
        let db = if read_only {
            StoreDb::open_readonly(&db_path)?
        } else {
            let db = StoreDb::open(&db_path, OpenMode::ReadWrite)?;
            configure_for_gc(&db)?;
            db
        };
        Ok(GcStore { db, layout })
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
    Ok(())
}
