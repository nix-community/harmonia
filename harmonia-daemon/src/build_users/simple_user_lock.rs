// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Group-based build users for macOS, matching Nix's `SimpleUserLock`.
//!
//! Iterates members of the `build-users-group` (typically `nixbld`)
//! and locks `<stateDir>/userpool/<uid>` for cross-process coordination.
//! Used on macOS where auto-allocate-uids is not available.

use std::ffi::CString;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use nix::unistd::{Gid, Uid, User, geteuid, getuid};

use super::UserLock;

/// Look up supplementary group IDs for a build user, excluding `primary_gid`.
///
/// On Linux, resolves the username from `uid` via `getpwuid` and calls
/// `getgrouplist(3)` — matching Nix's `get_group_list(pw->pw_name, pw->pw_gid)`.
/// Returns an empty list when no passwd entry exists for `uid`.
/// On non-Linux platforms, returns an empty list (Nix only does this on Linux).
#[cfg(target_os = "linux")]
fn get_supplementary_gids(uid: Uid, primary_gid: Gid) -> io::Result<Vec<Gid>> {
    let user = match User::from_uid(uid) {
        Ok(Some(u)) => u,
        Ok(None) => return Ok(Vec::new()),
        Err(e) => return Err(io::Error::other(e)),
    };

    let c_name = CString::new(user.name.as_bytes()).map_err(|e| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("username contains NUL: {e}"),
        )
    })?;

    let gids = nix::unistd::getgrouplist(&c_name, primary_gid)
        .map_err(|e| io::Error::other(format!("getgrouplist: {e}")))?;

    Ok(gids.into_iter().filter(|&g| g != primary_gid).collect())
}

#[cfg(not(target_os = "linux"))]
fn get_supplementary_gids(_uid: Uid, _primary_gid: Gid) -> io::Result<Vec<Gid>> {
    Ok(Vec::new())
}

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
    group_member_uids: &[(Uid, Gid)],
) -> std::io::Result<Option<UserLock>> {
    fs::create_dir_all(pool_dir)?;

    for &(uid, gid) in group_member_uids {
        let lock_path = pool_dir.join(uid.as_raw().to_string());

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
        if uid == getuid() || uid == geteuid() {
            return Err(io::Error::new(
                ErrorKind::PermissionDenied,
                format!("the Nix user should not be a member of the build users group (UID {uid})"),
            ));
        }

        // Gather supplementary groups (e.g. kvm) for this build user.
        // Matches Nix's get_group_list(pw->pw_name, pw->pw_gid) call.
        let supplementary_gids = get_supplementary_gids(uid, gid)?;

        return Ok(Some(UserLock {
            _fd: fd,
            first_uid: uid,
            first_gid: gid,
            nr_ids: 1,
            supplementary_gids,
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

        let my_uid = nix::unistd::getuid();
        let members = vec![(my_uid, Gid::from_raw(30000))];

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

        let my_uid = nix::unistd::getuid();
        let gid = Gid::from_raw(30000);
        // Put the self UID as the second member
        let members = vec![(Uid::from_raw(30001), gid), (my_uid, gid)];

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

        let gid = Gid::from_raw(30000);
        let members = vec![(Uid::from_raw(30001), gid), (Uid::from_raw(30002), gid)];

        let lock1 = acquire_simple_user_lock(&pool_dir, &members)
            .unwrap()
            .expect("user 0");
        assert_eq!(lock1.uid(), Uid::from_raw(30001));
        assert_eq!(lock1.gid(), gid);
        assert_eq!(lock1.uid_count(), 1);

        let lock2 = acquire_simple_user_lock(&pool_dir, &members)
            .unwrap()
            .expect("user 1");
        assert_eq!(lock2.uid(), Uid::from_raw(30002));

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
        assert_eq!(lock3.uid(), Uid::from_raw(30001));

        drop(lock2);
        drop(lock3);
    }
}
