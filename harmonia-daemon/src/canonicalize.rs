// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Path metadata canonicalization for build outputs.
//!
//! After a build, output paths must be canonicalized to ensure reproducibility:
//! - File permissions: clear group/world write bits
//! - Timestamps: set mtime to Unix epoch 1 (1970-01-01 00:00:01)
//! - Ownership: reset to root:root (UID 0, GID 0)

use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use nix::unistd::{Gid, Uid, geteuid};

/// Unix epoch + 1 second, matching Nix's canonical timestamp.
const EPOCH_PLUS_ONE: i64 = 1;

/// Set atime and mtime on `path` without following symlinks.
///
/// Uses `utimensat(AT_FDCWD, path, times, AT_SYMLINK_NOFOLLOW)` matching
/// Nix's `setWriteTime` implementation.
#[allow(unsafe_code)]
fn set_timestamp(path: &Path, seconds: i64) -> io::Result<()> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let times = [
        libc::timespec {
            tv_sec: seconds,
            tv_nsec: 0,
        },
        libc::timespec {
            tv_sec: seconds,
            tv_nsec: 0,
        },
    ];
    // SAFETY: c_path is a valid null-terminated string, times is a valid
    // 2-element array on the stack. AT_FDCWD makes the path interpreted
    // relative to cwd (i.e. as absolute when path is absolute).
    // AT_SYMLINK_NOFOLLOW prevents following symlinks.
    let ret = unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            c_path.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Canonicalize all metadata under `path` recursively.
///
/// - Clears group and world write bits on all files/dirs
/// - Sets mtime to epoch 1
/// - Sets ownership to root:root (UID 0, GID 0) if running as root
///
/// Runs the blocking filesystem walk on the tokio blocking pool.
pub async fn canonicalize_path_metadata(path: &Path) -> io::Result<()> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || canonicalize_path_metadata_sync(&path))
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
}

fn canonicalize_path_metadata_sync(path: &Path) -> io::Result<()> {
    canonicalize_entry(path)?;

    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            canonicalize_path_metadata_sync(&entry.path())?;
        }
    }

    Ok(())
}

fn canonicalize_entry(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;

    // Don't modify symlinks — they don't have independent permissions/timestamps
    if metadata.is_symlink() {
        return Ok(());
    }

    // Clear group and world write bits (keep owner permissions intact)
    let mode = metadata.permissions().mode();
    let new_mode = mode & !0o022; // clear group-write and other-write
    if new_mode != mode {
        fs::set_permissions(path, fs::Permissions::from_mode(new_mode))?;
    }

    // Set atime and mtime to epoch 1
    set_timestamp(path, EPOCH_PLUS_ONE)?;

    // Set ownership to root:root if we're running as root
    // (In tests we typically aren't root, so this is best-effort)
    if geteuid().is_root() {
        nix::unistd::chown(path, Some(Uid::from_raw(0)), Some(Gid::from_raw(0)))
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt};
    use tempfile::TempDir;

    /// File with mode 0777 → group/world write bits cleared after canonicalization.
    #[tokio::test]
    async fn test_permissions_canonicalized() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test_file");
        fs::write(&file, "hello").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o777)).unwrap();

        canonicalize_path_metadata(&file).await.unwrap();

        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o022,
            0,
            "Group and world write bits should be cleared, got {mode:o}"
        );
        // Owner bits preserved
        assert_ne!(mode & 0o700, 0, "Owner bits should be preserved");
    }

    /// File with current timestamp → mtime set to epoch 1 after canonicalization.
    #[tokio::test]
    async fn test_timestamps_reset() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test_file");
        fs::write(&file, "hello").unwrap();

        canonicalize_path_metadata(&file).await.unwrap();

        let metadata = fs::metadata(&file).unwrap();
        assert_eq!(
            metadata.mtime(),
            EPOCH_PLUS_ONE,
            "mtime should be set to epoch 1"
        );
    }

    /// File owned by build user UID → ownership reset to root:root after
    /// canonicalization (only testable when running as root).
    #[tokio::test]
    async fn test_ownership_reset() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test_file");
        fs::write(&file, "hello").unwrap();

        canonicalize_path_metadata(&file).await.unwrap();

        // We can only verify ownership change if running as root
        let metadata = fs::metadata(&file).unwrap();
        if nix::unistd::geteuid().is_root() {
            assert_eq!(metadata.uid(), 0, "UID should be 0 (root)");
            assert_eq!(metadata.gid(), 0, "GID should be 0 (root)");
        }
        // When not root, just verify the function didn't error
    }
}
