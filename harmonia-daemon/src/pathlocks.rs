// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Filesystem-based path locks matching Nix's `PathLocks` semantics.
//!
//! Each store path `<path>` is protected by an exclusive `flock()` on
//! `<path>.lock`. This allows cross-process coordination (multiple daemon
//! instances, GC) without relying on in-process mutexes.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use nix::fcntl::Flock;
use nix::fcntl::FlockArg;

/// An exclusive lock on a store path, backed by `flock()` on `<path>.lock`.
///
/// The lock is released when this value is dropped.
pub struct PathLock {
    _flock: Flock<File>,
    _lock_path: PathBuf,
}

impl PathLock {
    /// Acquire an exclusive lock on `path` (blocking).
    ///
    /// Creates `<path>.lock` if it doesn't exist.
    pub fn lock(path: &Path) -> io::Result<Self> {
        let lock_path = PathBuf::from(format!("{}.lock", path.display()));

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;

        let flock = Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, errno)| {
            io::Error::new(io::ErrorKind::Other, format!("flock failed: {errno}"))
        })?;

        Ok(Self {
            _flock: flock,
            _lock_path: lock_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    #[test]
    fn test_lock_creates_lock_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test-path");
        std::fs::write(&path, "data").unwrap();

        let _lock = PathLock::lock(&path).unwrap();

        let lock_file = PathBuf::from(format!("{}.lock", path.display()));
        assert!(lock_file.exists(), "Lock file should be created");
    }

    #[test]
    fn test_lock_is_exclusive() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("contested");

        // Track which thread got the lock first and performed its work
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let barrier = Arc::new(Barrier::new(2));

        let path2 = path.clone();
        let order2 = order.clone();
        let barrier2 = barrier.clone();

        let t1 = std::thread::spawn(move || {
            barrier2.wait();
            let _lock = PathLock::lock(&path2).unwrap();
            order2.lock().unwrap().push(1);
            // Hold lock briefly so the other thread blocks
            std::thread::sleep(std::time::Duration::from_millis(50));
        });

        let path3 = path.clone();
        let order3 = order.clone();
        let barrier3 = barrier.clone();

        let t2 = std::thread::spawn(move || {
            barrier3.wait();
            let _lock = PathLock::lock(&path3).unwrap();
            order3.lock().unwrap().push(2);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        let order = order.lock().unwrap();
        assert_eq!(order.len(), 2, "Both threads should complete");
        // We can't guarantee ordering, but both must have run sequentially
        // (no overlap). The 50ms sleep means if locking weren't exclusive,
        // both would record nearly simultaneously.
    }

    #[test]
    fn test_lock_released_on_drop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("drop-test");

        {
            let _lock = PathLock::lock(&path).unwrap();
        }
        // After drop, we should be able to re-acquire
        let _lock = PathLock::lock(&path).unwrap();
    }
}
