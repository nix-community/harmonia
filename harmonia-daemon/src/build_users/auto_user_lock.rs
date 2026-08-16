// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Auto-allocated build users for Linux, matching Nix's `AutoUserLock`.
//!
//! Divides a UID range into slots of 65536 (`maxIdsPerBuild`) and uses
//! file locks in `<stateDir>/userpool2/slot-<N>` for cross-process
//! coordination.

use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::unistd::{Uid, User};

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
            Err((_, Errno::EWOULDBLOCK | Errno::EINTR)) => continue, // Slot is busy
            Err((_, errno)) => return Err(errno.into()),             // Real error
        };

        let first_uid = Uid::from_raw(start_id + i * MAX_IDS_PER_BUILD);

        // Safety: reject UIDs that collide with real system users.
        // Matches Nix's `getpwuid(firstUid)` check in AutoUserLock::acquire.
        if let Ok(Some(user)) = User::from_uid(first_uid) {
            return Err(io::Error::new(
                ErrorKind::AddrInUse,
                format!(
                    "auto-allocated UID {} clashes with existing user account '{}'",
                    first_uid, user.name
                ),
            ));
        }

        return Ok(Some(UserLock {
            _fd: fd,
            first_uid,
            first_gid: nix::unistd::Gid::from_raw(first_uid.as_raw()),
            nr_ids,
            supplementary_gids: Vec::new(),
        }));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// If the auto-allocated first_uid belongs to an existing system user
    /// the function must return an error, not silently hand out that UID.
    /// Uses the current process's UID which is guaranteed to exist in
    /// /etc/passwd and is always > 0 (tests never run as root).
    #[test]
    fn test_uid_clash_check() {
        let tmp = TempDir::new().unwrap();
        let pool_dir = tmp.path().join("userpool2");

        let my_uid = nix::unistd::getuid().as_raw();
        assert!(my_uid > 0, "this test cannot run as root");

        let result = acquire_auto_user_lock(&pool_dir, my_uid, MAX_IDS_PER_BUILD, 1);
        assert!(
            result.is_err(),
            "should fail: UID {my_uid} clashes with current user"
        );
    }

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
        assert_eq!(lock1.uid(), Uid::from_raw(30000));
        assert_eq!(lock2.uid(), Uid::from_raw(30000 + MAX_IDS_PER_BUILD));
        assert_eq!(lock1.gid().as_raw(), lock1.uid().as_raw());
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
        assert_eq!(
            lock.uid_range(),
            Some((Uid::from_raw(30000), Uid::from_raw(30000 + 65535)))
        );
    }
}
