// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Test helper providing a self-contained Nix store backed by an in-memory
//! SQLite database and a temporary directory on disk.
//!
//! Does **not** depend on `nix-store` or any external Nix tooling.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use harmonia_store_core::store_path::StoreDir;
use harmonia_store_db::StoreDb;
use harmonia_utils_test::CanonicalTempDir;

use harmonia_store_core::signature::PublicKey;

use crate::handler::LocalStoreHandler;

/// A self-contained test store.
///
/// Owns a temporary directory (the store root) and an in-memory SQLite
/// database initialised with the full Nix schema.  Provides both raw DB
/// access (for test setup) and a [`LocalStoreHandler`] that queries the
/// same database (for exercising the daemon protocol).
pub struct TestStore {
    pub store_dir: StoreDir,
    pub handler: LocalStoreHandler,
    pub db: Arc<Mutex<StoreDb>>,
    _temp_dir: CanonicalTempDir,
}

impl TestStore {
    /// Create a new test store with a fresh temp directory and in-memory DB.
    pub fn new() -> Self {
        let temp_dir = CanonicalTempDir::new().expect("failed to create temp dir");
        let store_path = temp_dir.path().join("store");
        std::fs::create_dir_all(&store_path).expect("failed to create store dir");

        let store_dir =
            StoreDir::new(&store_path).expect("failed to create StoreDir from temp path");
        let db = StoreDb::open_memory().expect("failed to create in-memory database");
        let db = Arc::new(Mutex::new(db));

        let build_dir = temp_dir.path().join("builds");
        std::fs::create_dir_all(&build_dir).expect("failed to create build dir");
        let mut handler = LocalStoreHandler::from_shared_db(store_dir.clone(), Arc::clone(&db));
        handler.set_build_dir(build_dir);

        Self {
            store_dir,
            db,
            handler,
            _temp_dir: temp_dir,
        }
    }

    /// Create a new test store with trusted public keys for signature verification.
    pub fn with_trusted_keys(keys: Vec<PublicKey>) -> Self {
        let temp_dir = CanonicalTempDir::new().expect("failed to create temp dir");
        let store_path = temp_dir.path().join("store");
        std::fs::create_dir_all(&store_path).expect("failed to create store dir");

        let store_dir =
            StoreDir::new(&store_path).expect("failed to create StoreDir from temp path");
        let db = StoreDb::open_memory().expect("failed to create in-memory database");
        let db = Arc::new(Mutex::new(db));

        let build_dir = temp_dir.path().join("builds");
        std::fs::create_dir_all(&build_dir).expect("failed to create build dir");
        let mut handler =
            LocalStoreHandler::from_shared_db_with_keys(store_dir.clone(), Arc::clone(&db), keys);
        handler.set_build_dir(build_dir);

        Self {
            store_dir,
            db,
            handler,
            _temp_dir: temp_dir,
        }
    }

    /// Filesystem path to the store root (e.g. `/tmp/xxx/store`).
    pub fn store_path(&self) -> PathBuf {
        self.store_dir.to_path().to_owned()
    }

    /// Directory for temporary build sandboxes, created under the test temp dir.
    pub fn build_dir(&self) -> PathBuf {
        let p = self._temp_dir.path().join("builds");
        std::fs::create_dir_all(&p).expect("failed to create build dir");
        p
    }
}
