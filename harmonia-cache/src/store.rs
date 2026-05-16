use crate::error::{CacheError, Result, StoreError};
use harmonia_store_db::{Realisation, StoreDb, ValidPathInfo};
use harmonia_store_path::{StoreDir, StorePath};
use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

// rusqlite::Connection is !Sync, so each actix worker thread keeps its own
// handle to the nix database. Opened lazily on first use.
thread_local! {
    static LOCAL_DB: RefCell<Option<StoreDb>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct Store {
    store_dir: StoreDir,
    real_store: Option<PathBuf>,
    db_path: Arc<PathBuf>,
}

impl Store {
    pub fn new(store_dir: StoreDir, real_store: Option<PathBuf>, db_path: PathBuf) -> Self {
        Self {
            store_dir,
            real_store,
            db_path: Arc::new(db_path),
        }
    }

    /// The on-disk store directory (may differ from `store_dir` in chroot setups).
    pub fn real_store(&self) -> &Path {
        self.real_store
            .as_deref()
            .unwrap_or(self.store_dir.as_ref())
    }

    pub fn get_real_path(&self, store_path: &StorePath) -> PathBuf {
        self.real_store().join(store_path.to_string())
    }

    pub fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    /// Run `f` against this thread's SQLite handle to the nix database.
    fn with_db<R>(&self, f: impl FnOnce(&StoreDb) -> Result<R>) -> Result<R> {
        LOCAL_DB.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                let db =
                    StoreDb::open_readonly(self.db_path.as_path()).map_err(|e| StoreError::Db {
                        path: self.db_path.display().to_string(),
                        reason: e.to_string(),
                    })?;
                *slot = Some(db);
            }
            f(slot.as_ref().expect("db just opened"))
        })
    }

    fn db_err(&self, e: harmonia_store_db::Error) -> CacheError {
        StoreError::Db {
            path: self.db_path.display().to_string(),
            reason: e.to_string(),
        }
        .into()
    }

    /// Resolve a 32-char store-path hash to its full `StorePath`.
    pub fn query_path_from_hash_part(
        &self,
        hash: &harmonia_store_path::StorePathHash,
    ) -> Result<Option<StorePath>> {
        self.with_db(|db| {
            db.query_path_from_hash_part(&self.store_dir, hash)
                .map_err(|e| self.db_err(e))
        })
    }

    /// Resolve a 32-char store-path hash directly to its `ValidPathInfo`.
    pub fn query_path_info_by_hash_part(
        &self,
        hash: &harmonia_store_path::StorePathHash,
    ) -> Result<Option<ValidPathInfo>> {
        self.with_db(|db| {
            db.query_path_info_by_hash_part(&self.store_dir, hash)
                .map_err(|e| self.db_err(e))
        })
    }

    pub fn is_valid_path(&self, path: &StorePath) -> Result<bool> {
        self.with_db(|db| {
            db.is_valid_path(&self.store_dir, path)
                .map_err(|e| self.db_err(e))
        })
    }

    /// Look up a CA-derivation realisation. Returns `None` both when the row
    /// is absent and when the database has no `BuildTraceV3` table (i.e. CA
    /// derivations were never enabled on this store).
    pub fn query_realisation(
        &self,
        drv_path: &StorePath,
        output_name: &harmonia_store_core::derived_path::OutputName,
    ) -> Result<Option<Realisation>> {
        self.with_db(|db| {
            if !db.has_ca_schema().map_err(|e| self.db_err(e))? {
                return Ok(None);
            }
            db.query_realisation(&self.store_dir, drv_path, output_name)
                .map_err(|e| self.db_err(e))
        })
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new(
            StoreDir::default(),
            None,
            PathBuf::from("/nix/var/nix/db/db.sqlite"),
        )
    }
}
