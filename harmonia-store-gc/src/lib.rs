// SPDX-FileCopyrightText: 2025 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Garbage collection for the Nix store.
//!
//! A faster drop-in replacement for `nix-collect-garbage`. Instead of one
//! SQLite query per store path, the reference graph is loaded once into
//! memory (see [`harmonia_store_db::StoreGraph`]) and liveness is computed
//! as a graph closure. Deletion runs in parallel.
//!
//! The crate speaks the same on-disk protocols as Nix itself. It takes
//! the same `gc.lock`, honors the temp-root files that running builds
//! register, and serves the `gc-socket` that builders use to protect new
//! paths while a collection is in progress. Running it next to a busy
//! nix-daemon is safe.
//!
//! Entry point: [`gc::collect_garbage`] with a [`store::GcStore`].

pub mod config;
mod error;
pub mod store;

pub use error::{Error, Result};

use std::path::Path;

/// Hash map keyed by store path strings.
///
/// SipHash showed up in GC profiles when hashing millions of ~50-char
/// store paths, so foldhash is used instead.
pub type HashMap<K, V> = std::collections::HashMap<K, V, foldhash::fast::RandomState>;
/// Hash set counterpart of [`HashMap`].
pub type HashSet<K> = std::collections::HashSet<K, foldhash::fast::RandomState>;

/// Format a byte count for human-readable log output.
///
/// ```
/// assert_eq!(harmonia_store_gc::format_size(1536), "1.50 KiB");
/// ```
pub fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.2} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.2} KiB", b / KIB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Enter a private mount namespace so the read-write remount in
/// [`make_store_writable`] stays scoped to this process, mirroring what
/// the `nix` CLI does for root.
///
/// Must be called from `main` before any thread pool starts. Only the
/// calling thread joins the new namespace, so worker threads created
/// earlier would keep seeing the read-only store.
///
/// Failures are logged and ignored (e.g. EPERM in containers). We then
/// fall back to remounting in the host namespace, like legacy `nix-store`.
#[cfg(target_os = "linux")]
pub fn unshare_mount_namespace() {
    use nix::mount::{MsFlags, mount};
    use nix::sched::{CloneFlags, unshare};
    use nix::unistd::Uid;

    if !Uid::effective().is_root() {
        return;
    }
    if let Err(e) = unshare(CloneFlags::CLONE_NEWNS) {
        tracing::warn!("failed to set up a private mount namespace: {e}");
        return;
    }
    // Default propagation on systemd is `shared`. Without marking /
    // private, the remount would propagate back to the host namespace.
    if let Err(e) = mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_PRIVATE | MsFlags::MS_REC,
        None::<&str>,
    ) {
        tracing::warn!("failed to mark / private in mount namespace: {e}");
    }
}

#[cfg(not(target_os = "linux"))]
pub fn unshare_mount_namespace() {}

/// Remount the store read-write if needed. NixOS bind-mounts /nix/store
/// read-only, so this must run before any deletion.
#[cfg(target_os = "linux")]
pub fn make_store_writable(real_store_dir: &Path) -> Result<()> {
    use nix::mount::{MsFlags, mount};
    use nix::sys::statvfs::{FsFlags, statvfs};
    use nix::unistd::Uid;

    if !Uid::effective().is_root() {
        return Ok(());
    }

    let st = statvfs(real_store_dir).map_err(|source| Error::MountInfo {
        path: real_store_dir.to_owned(),
        source,
    })?;
    if !st.flags().contains(FsFlags::ST_RDONLY) {
        return Ok(());
    }

    // Preserve locked mount flags (nodev etc.), otherwise the remount
    // fails in a user namespace.
    let mut flags = MsFlags::MS_REMOUNT | MsFlags::MS_BIND;
    let f = st.flags();
    for (fs_flag, ms_flag) in [
        (FsFlags::ST_NODEV, MsFlags::MS_NODEV),
        (FsFlags::ST_NOSUID, MsFlags::MS_NOSUID),
        (FsFlags::ST_NOEXEC, MsFlags::MS_NOEXEC),
        (FsFlags::ST_NOATIME, MsFlags::MS_NOATIME),
        (FsFlags::ST_NODIRATIME, MsFlags::MS_NODIRATIME),
        (FsFlags::ST_RELATIME, MsFlags::MS_RELATIME),
        (FsFlags::ST_SYNCHRONOUS, MsFlags::MS_SYNCHRONOUS),
    ] {
        if f.contains(fs_flag) {
            flags |= ms_flag;
        }
    }

    mount(
        None::<&str>,
        real_store_dir,
        None::<&str>,
        flags,
        None::<&str>,
    )
    .map_err(|source| Error::Remount {
        path: real_store_dir.to_owned(),
        source,
    })?;
    tracing::info!("remounted {} read-write", real_store_dir.display());
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn make_store_writable(_real_store_dir: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::format_size;

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(0), "0 bytes");
        assert_eq!(format_size(1023), "1023 bytes");
        assert_eq!(format_size(1024), "1.00 KiB");
        assert_eq!(format_size(1536), "1.50 KiB");
        assert_eq!(format_size(1024 * 1024), "1.00 MiB");
        assert_eq!(format_size(5 * 1024 * 1024 + 512 * 1024), "5.50 MiB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GiB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024 / 2), "1.50 GiB");
    }
}
