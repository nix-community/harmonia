// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Group-based build users for macOS, matching Nix's `SimpleUserLock`.
//!
//! Iterates members of the `build-users-group` (typically `nixbld`)
//! and locks `<stateDir>/userpool/<uid>` for cross-process coordination.
//! Used on macOS where auto-allocate-uids is not available.

use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::unistd::{geteuid, getuid};

use super::UserLock;

/// Resolve the userpool directory (matches Nix's simple lock path).
pub fn simple_pool_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("userpool")
}

/// Acquire a build user from a list of group member (uid, gid) pairs.
///
/// Tries to lock `<pool_dir>/<uid>` for each member. Returns `None`
/// if all members are busy.
pub fn acquire_simple_user_lock(
    pool_dir: &Path,
    group_member_uids: &[(u32, u32)], // (uid, gid) pairs
) -> std::io::Result<Option<UserLock>> {
    fs::create_dir_all(pool_dir)?;

    for &(uid, gid) in group_member_uids {
        let lock_path = pool_dir.join(uid.to_string());

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)?;

        let fd = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(fd) => fd,
            Err((_, Errno::EWOULDBLOCK | Errno::EINTR)) => continue, // Slot is busy
            Err((_, errno)) => return Err(errno.into()),             // Real error
        };

        // Safety: the Nix daemon must never run builds as itself.
        // Matches Nix's `lock->uid == getuid() || lock->uid == geteuid()` check.
        if uid == getuid().as_raw() || uid == geteuid().as_raw() {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                format!("the Nix user should not be a member of the build users group (UID {uid})"),
            ));
        }

        return Ok(Some(UserLock {
            _fd: fd,
            first_uid: uid,
            first_gid: gid,
            nr_ids: 1,
        }));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// The daemon must never hand out its own UID as a build user.
    /// Nix checks `lock->uid == getuid() || lock->uid == geteuid()` and
    /// throws an error. Verify that acquire_simple_user_lock rejects
    /// members whose UID matches the current process.
    #[test]
    fn test_self_uid_rejected() {
        let tmp = TempDir::new().unwrap();
        let pool_dir = tmp.path().join("userpool");

        let my_uid = nix::unistd::getuid().as_raw();
        let members = vec![(my_uid, 30000_u32)];

        let result = acquire_simple_user_lock(&pool_dir, &members);
        assert!(
            result.is_err(),
            "should reject UID {my_uid} — it is the daemon's own UID"
        );
    }

    /// If the self-UID appears among several members, the function must
    /// return an error (matching Nix which throws rather than skipping).
    #[test]
    fn test_self_uid_among_others_is_error() {
        let tmp = TempDir::new().unwrap();
        let pool_dir = tmp.path().join("userpool");

        let my_uid = nix::unistd::getuid().as_raw();
        // Put the self UID as the second member
        let members = vec![(30001_u32, 30000_u32), (my_uid, 30000)];

        // First lock grabs 30001 successfully
        let _lock1 = acquire_simple_user_lock(&pool_dir, &members)
            .unwrap()
            .expect("user 30001");

        // Second attempt hits our own UID → must error
        let result = acquire_simple_user_lock(&pool_dir, &members);
        assert!(
            result.is_err(),
            "should reject UID {my_uid} — it is the daemon's own UID"
        );
    }

    #[test]
    fn test_acquires_and_releases() {
        let tmp = TempDir::new().unwrap();
        let pool_dir = tmp.path().join("userpool");

        let members = vec![(30001_u32, 30000_u32), (30002, 30000)];

        let lock1 = acquire_simple_user_lock(&pool_dir, &members)
            .unwrap()
            .expect("user 0");
        assert_eq!(lock1.uid(), 30001);
        assert_eq!(lock1.gid(), 30000);
        assert_eq!(lock1.uid_count(), 1);

        let lock2 = acquire_simple_user_lock(&pool_dir, &members)
            .unwrap()
            .expect("user 1");
        assert_eq!(lock2.uid(), 30002);

        // Exhausted
        assert!(
            acquire_simple_user_lock(&pool_dir, &members)
                .unwrap()
                .is_none()
        );

        // Release first
        drop(lock1);
        let lock3 = acquire_simple_user_lock(&pool_dir, &members)
            .unwrap()
            .expect("reacquire");
        assert_eq!(lock3.uid(), 30001);

        drop(lock2);
        drop(lock3);
    }
}
