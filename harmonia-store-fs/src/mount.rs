// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Remounting a read-only store for mutation. Only Linux bind-mounts the
//! store, so the other platforms are no-ops.

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    use nix::mount::{MsFlags, mount};
    use nix::sched::{CloneFlags, unshare};
    use nix::sys::statvfs::{FsFlags, statvfs};
    use nix::unistd::Uid;

    use crate::Error;

    /// Enter a private mount namespace so the read-write remount in
    /// [`make_store_writable`] stays scoped to this process, mirroring
    /// what the `nix` CLI does for root.
    ///
    /// Must be called from `main` before any thread pool starts: only the
    /// calling thread joins the new namespace.
    ///
    /// Errors are reported, not acted on: EPERM is normal in containers,
    /// and the caller can still remount in the host namespace like
    /// legacy `nix-store` does.
    pub fn unshare_mount_namespace() -> Result<(), Error> {
        if !Uid::effective().is_root() {
            tracing::debug!("not root, skipping private mount namespace");
            return Ok(());
        }
        unshare(CloneFlags::CLONE_NEWNS).map_err(|source| Error::Unshare { source })?;
        // Default propagation on systemd is `shared`. Without marking /
        // private, the remount would propagate back to the host
        // namespace.
        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_PRIVATE | MsFlags::MS_REC,
            None::<&str>,
        )
        .map_err(|source| Error::Unshare { source })?;
        Ok(())
    }

    /// Remount the store read-write if needed. NixOS bind-mounts
    /// /nix/store read-only, so this must run before any store mutation.
    pub fn make_store_writable(real_store_dir: &Path) -> Result<(), Error> {
        if !Uid::effective().is_root() {
            tracing::debug!("not root, skipping read-write remount of the store");
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
        // fails in a user namespace. Flags that neither nix nor libc can
        // report from statvfs (e.g. nosymfollow) cannot be preserved.
        // For the store that is moot: it is full of symlinks, a
        // nosymfollow mount would break Nix itself.
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
}

#[cfg(target_os = "linux")]
pub use linux::{make_store_writable, unshare_mount_namespace};

#[cfg(not(target_os = "linux"))]
mod other {
    use std::path::Path;

    use crate::Error;

    /// No-op outside Linux.
    pub fn unshare_mount_namespace() -> Result<(), Error> {
        Ok(())
    }

    /// No-op outside Linux.
    pub fn make_store_writable(_real_store_dir: &Path) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(not(target_os = "linux"))]
pub use other::{make_store_writable, unshare_mount_namespace};
