// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Filesystem layout of a local Nix store.
//!
//! A local store is more than the store directory. Nix keeps its
//! database, locks and GC state in a separate state directory, and the
//! store directory itself can be a symlink or bind mount whose physical
//! location differs from the `/nix/store` paths recorded in the
//! database. [`StoreLayout`] resolves all of these once, so tools that
//! modify the store (garbage collection, store optimisation) agree on
//! where things are:
//!
//! - the logical store directory, as recorded in the database
//! - the physical directory behind it, for actual disk operations
//! - the state directory with the database, `gc.lock` and temp roots
//! - the `.links` directory holding one hard link per unique file for
//!   deduplication
//!
//! On NixOS `/nix/store` is bind-mounted read-only so nothing tampers
//! with it by accident. Store mutation must undo that for itself:
//! [`unshare_mount_namespace`] gives the process a private mount
//! namespace, and [`make_store_writable`] remounts the store read-write
//! inside it, invisible to the rest of the system.

use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

mod mount;
pub use mount::{make_store_writable, unshare_mount_namespace};

use harmonia_store_path::StoreDir;
pub use harmonia_store_path::StoreDirError;

/// A filesystem operation on the store failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The store directory is not a valid absolute path
    #[error(transparent)]
    StoreDir(#[from] StoreDirError),

    /// Querying mount information for the store failed
    #[cfg(target_os = "linux")]
    #[error("getting mount info for {path}: {source}")]
    MountInfo { path: PathBuf, source: nix::Error },

    /// Remounting the store read-write failed
    #[cfg(target_os = "linux")]
    #[error("remounting {path} writable: {source}")]
    Remount { path: PathBuf, source: nix::Error },

    /// Setting up a private mount namespace failed
    #[cfg(target_os = "linux")]
    #[error("setting up a private mount namespace: {source}")]
    Unshare { source: nix::Error },
}

/// Where a local Nix store lives on disk.
///
/// ```
/// use std::path::Path;
/// use harmonia_store_fs::StoreLayout;
///
/// let layout = StoreLayout::new(Path::new("/nix/store/"), Path::new("/nix/var/nix"))?;
/// assert_eq!(layout.store_dir().to_str(), "/nix/store");
/// assert_eq!(layout.db_path(), Path::new("/nix/var/nix/db/db.sqlite"));
/// # Ok::<(), harmonia_store_fs::Error>(())
/// ```
pub struct StoreLayout {
    store_dir: StoreDir,
    real_store_dir: PathBuf,
    state_dir: PathBuf,
    links_dir: PathBuf,
}

impl StoreLayout {
    /// Resolve the layout for the store at `store_dir` with state in
    /// `state_dir`.
    ///
    /// Falls back to the logical dir when it cannot be canonicalized,
    /// like Nix's `realStoreDir`.
    pub fn new(store_dir: &Path, state_dir: &Path) -> Result<Self, Error> {
        let store_dir = StoreDir::new(normalize_dir(store_dir))?;
        let real_store_dir = fs::canonicalize(store_dir.to_path()).unwrap_or_else(|e| {
            tracing::debug!(
                "cannot canonicalize {}: {e}, using it as-is",
                store_dir.to_str()
            );
            store_dir.to_path().into()
        });
        Ok(StoreLayout {
            links_dir: real_store_dir.join(".links"),
            real_store_dir,
            state_dir: state_dir.to_owned(),
            store_dir,
        })
    }

    /// The logical store directory, e.g. `/nix/store`. This is the form
    /// recorded in the database.
    pub fn store_dir(&self) -> &StoreDir {
        &self.store_dir
    }

    /// The physical store directory: [`Self::store_dir`] with symlinks
    /// resolved. The database records logical paths, but disk operations
    /// need real ones.
    pub fn real_store_dir(&self) -> &Path {
        &self.real_store_dir
    }

    /// The Nix state directory, e.g. `/nix/var/nix`. Holds `gc.lock`,
    /// `temproots/`, `gc-socket/` and the database.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// The hard-link dedup directory `real_store_dir/.links`.
    pub fn links_dir(&self) -> &Path {
        &self.links_dir
    }

    /// The store database file `state_dir/db/db.sqlite`.
    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("db/db.sqlite")
    }
}

/// Strip trailing slashes but keep a bare "/". A trailing slash would
/// break every store prefix comparison downstream.
fn normalize_dir(dir: &Path) -> PathBuf {
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
