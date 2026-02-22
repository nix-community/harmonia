// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Auto-allocated build users for Linux, matching Nix's `AutoUserLock`.
//!
//! Divides a UID range into slots of 65536 (`maxIdsPerBuild`) and uses
//! file locks in `<stateDir>/userpool2/slot-<N>` for cross-process
//! coordination.

use std::fs;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::fcntl::{Flock, FlockArg};

use super::UserLock;

/// Maximum UIDs per build slot on Linux (2^16 for full uid_map range).
/// Matches Nix's `maxIdsPerBuild`.
pub const MAX_IDS_PER_BUILD: u32 = 1 << 16;

/// Resolve the userpool2 directory (matches Nix's auto-allocate path).
pub fn auto_pool_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("userpool2")
}

/// Acquire a build user via auto-allocated UID slots.
///
/// Divides `id_count` UIDs starting at `start_id` into slots of
/// `MAX_IDS_PER_BUILD`. Tries each slot's lock file in
/// `<pool_dir>/slot-<N>`. Returns `None` if all slots are busy.
///
/// With user namespaces, GID == UID (mapped inside the namespace).
pub fn acquire_auto_user_lock(
    pool_dir: &Path,
    start_id: u32,
    id_count: u32,
    nr_ids: u32,
) -> std::io::Result<Option<UserLock>> {
    assert!(start_id > 0);
    assert!(id_count.is_multiple_of(MAX_IDS_PER_BUILD));
    assert!(
        (start_id as u64) + (id_count as u64) <= u32::MAX as u64,
        "UID range overflows u32"
    );
    assert!(nr_ids <= MAX_IDS_PER_BUILD);

    fs::create_dir_all(pool_dir)?;

    let nr_slots = id_count / MAX_IDS_PER_BUILD;

    for i in 0..nr_slots {
        let lock_path = pool_dir.join(format!("slot-{i}"));

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)?;

        // Try non-blocking exclusive lock
        let fd = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(fd) => fd,
            Err(_) => continue, // Slot is busy
        };

        let first_uid = start_id + i * MAX_IDS_PER_BUILD;

        return Ok(Some(UserLock {
            _fd: fd,
            first_uid,
            first_gid: first_uid,
            nr_ids,
        }));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_acquires_distinct_slots() {
        let tmp = TempDir::new().unwrap();
        let pool_dir = tmp.path().join("userpool2");

        let lock1 = acquire_auto_user_lock(&pool_dir, 30000, MAX_IDS_PER_BUILD * 4, 1)
            .unwrap()
            .expect("slot 0");
        let lock2 = acquire_auto_user_lock(&pool_dir, 30000, MAX_IDS_PER_BUILD * 4, 1)
            .unwrap()
            .expect("slot 1");

        assert_ne!(lock1.uid(), lock2.uid());
        assert_eq!(lock1.uid(), 30000);
        assert_eq!(lock2.uid(), 30000 + MAX_IDS_PER_BUILD);
        assert_eq!(lock1.gid(), lock1.uid());
    }

    #[test]
    fn test_exhaustion_and_release() {
        let tmp = TempDir::new().unwrap();
        let pool_dir = tmp.path().join("userpool2");

        // Only 2 slots
        let lock1 = acquire_auto_user_lock(&pool_dir, 30000, MAX_IDS_PER_BUILD * 2, 1)
            .unwrap()
            .expect("slot 0");
        let lock2 = acquire_auto_user_lock(&pool_dir, 30000, MAX_IDS_PER_BUILD * 2, 1)
            .unwrap()
            .expect("slot 1");

        // Exhausted
        assert!(
            acquire_auto_user_lock(&pool_dir, 30000, MAX_IDS_PER_BUILD * 2, 1)
                .unwrap()
                .is_none()
        );

        // Release slot 0
        let released_uid = lock1.uid();
        drop(lock1);

        // Can acquire again — gets the same slot back
        let lock3 = acquire_auto_user_lock(&pool_dir, 30000, MAX_IDS_PER_BUILD * 2, 1)
            .unwrap()
            .expect("reacquire slot 0");
        assert_eq!(lock3.uid(), released_uid);

        drop(lock2);
        drop(lock3);
    }

    #[test]
    fn test_uid_range() {
        let tmp = TempDir::new().unwrap();
        let pool_dir = tmp.path().join("userpool2");

        let lock = acquire_auto_user_lock(&pool_dir, 30000, MAX_IDS_PER_BUILD * 4, 65536)
            .unwrap()
            .expect("slot 0");

        assert_eq!(lock.uid_count(), 65536);
        assert_eq!(lock.uid_range(), Some((30000, 30000 + 65535)));
    }
}
