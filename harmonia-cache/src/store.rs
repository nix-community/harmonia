use crate::error::{CacheError, Result, StoreError};
use harmonia_store_core::store_path::{FromStoreDirStr, StoreDir, StorePath};
use harmonia_store_db::{Realisation, StoreDb, ValidPathInfo};
use std::cell::RefCell;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
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
    virtual_store: Vec<u8>,
    real_store: Option<Vec<u8>>,
    store_dir: StoreDir,
    db_path: Arc<PathBuf>,
}

impl Store {
    pub fn new(virtual_store: Vec<u8>, real_store: Option<Vec<u8>>, db_path: PathBuf) -> Self {
        // The database stores paths under the virtual store prefix even in
        // chroot setups, so that is what we parse against.
        let store_dir_str = std::str::from_utf8(&virtual_store).unwrap_or("/nix/store");
        let store_dir = StoreDir::new(store_dir_str).unwrap_or_default();

        Self {
            virtual_store,
            real_store,
            store_dir,
            db_path: Arc::new(db_path),
        }
    }

    pub fn get_real_path(&self, store_path: &StorePath) -> PathBuf {
        // StorePath is now just "hash-name", construct full path
        let virtual_store_path = Path::new(OsStr::from_bytes(&self.virtual_store));
        let full_virtual_path = virtual_store_path.join(store_path.to_string());

        if self.real_store.is_some() {
            // Map from virtual store to real store
            return self.real_store().join(store_path.to_string());
        }
        full_virtual_path
    }

    pub fn real_store(&self) -> &Path {
        Path::new(OsStr::from_bytes(
            self.real_store.as_ref().unwrap_or(&self.virtual_store),
        ))
    }

    pub fn virtual_store(&self) -> &[u8] {
        &self.virtual_store
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

    fn parse_path(&self, full: &str) -> Result<StorePath> {
        StorePath::from_store_dir_str(&self.store_dir, full).map_err(|e| {
            StoreError::Db {
                path: self.db_path.display().to_string(),
                reason: format!("invalid store path '{full}': {e}"),
            }
            .into()
        })
    }

    /// Resolve a 32-char store-path hash to its full `StorePath`.
    pub fn query_path_from_hash_part(&self, hash: &str) -> Result<Option<StorePath>> {
        let store_dir = self.store_dir.to_str().to_owned();
        let full = self.with_db(|db| {
            db.query_path_from_hash_part(&store_dir, hash)
                .map_err(|e| self.db_err(e))
        })?;
        full.map(|p| self.parse_path(&p)).transpose()
    }

    /// Resolve a 32-char store-path hash directly to its `ValidPathInfo`.
    pub fn query_path_info_by_hash_part(&self, hash: &str) -> Result<Option<ValidPathInfo>> {
        let store_dir = self.store_dir.to_str().to_owned();
        self.with_db(|db| {
            db.query_path_info_by_hash_part(&store_dir, hash)
                .map_err(|e| self.db_err(e))
        })
    }

    pub fn is_valid_path(&self, path: &StorePath) -> Result<bool> {
        let full = format!("{}", self.store_dir.display(path));
        self.with_db(|db| db.is_valid_path(&full).map_err(|e| self.db_err(e)))
    }

    /// Look up a CA-derivation realisation. Returns `None` both when the row
    /// is absent and when the database has no `BuildTraceV3` table (i.e. CA
    /// derivations were never enabled on this store).
    pub fn query_realisation(
        &self,
        drv_base_name: &str,
        output_name: &str,
    ) -> Result<Option<Realisation>> {
        self.with_db(|db| {
            if !db.has_ca_schema().map_err(|e| self.db_err(e))? {
                return Ok(None);
            }
            db.query_realisation(drv_base_name, output_name)
                .map_err(|e| self.db_err(e))
        })
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new(
            b"/nix/store".to_vec(),
            None,
            PathBuf::from("/nix/var/nix/db/db.sqlite"),
        )
    }
}
