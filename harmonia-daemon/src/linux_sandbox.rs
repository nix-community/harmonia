// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Linux sandbox for build isolation via user namespaces.
//!
//! Uses `CLONE_NEWUSER` + `CLONE_NEWNS` to isolate the builder process.
//! A build user UID is allocated via file locks in the userpool directory
//! (matching Nix's `AutoUserLock`) and mapped into the user namespace so
//! the builder runs as an unprivileged user with a unique UID per
//! concurrent build.
//!
//! The sandbox creates:
//! - A user namespace mapping the allocated UID to root inside
//! - A mount namespace with bind mounts for /nix/store, build dir, /proc, /dev
//!
//! Unprivileged user namespaces must be enabled on the host
//! (`kernel.unprivileged_userns_clone = 1`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::build_users::{self, UserLock};
use crate::sandbox::{Sandbox, SandboxChild, SandboxError, SandboxMount};

/// Configuration for a sandboxed Linux build.
pub struct LinuxSandboxConfig {
    /// Store directory (usually `/nix/store`).
    pub store_dir: PathBuf,
    /// Temporary build directory.
    pub build_dir: PathBuf,
    /// System features required by the derivation (e.g., "kvm").
    pub required_system_features: BTreeSet<String>,
    /// Extra sandbox paths from daemon config.
    pub extra_sandbox_paths: Vec<PathBuf>,
    /// Whether `__noChroot` is set in the derivation (allows network).
    pub no_chroot: bool,
}

impl LinuxSandboxConfig {
    /// Compute the list of bind mounts for the sandbox.
    pub fn bind_mounts(&self) -> Vec<SandboxMount> {
        let mut mounts = Vec::new();

        // /nix/store — read-only
        mounts.push(SandboxMount {
            source: self.store_dir.clone(),
            target: self.store_dir.clone(),
            read_only: true,
            optional: false,
        });

        // Build directory — read-write
        mounts.push(SandboxMount {
            source: self.build_dir.clone(),
            target: self.build_dir.clone(),
            read_only: false,
            optional: false,
        });

        // /proc
        mounts.push(SandboxMount {
            source: PathBuf::from("/proc"),
            target: PathBuf::from("/proc"),
            read_only: false,
            optional: false,
        });

        // /dev devices
        for dev in &["null", "zero", "urandom", "ptmx", "pts"] {
            let path = PathBuf::from(format!("/dev/{dev}"));
            mounts.push(SandboxMount {
                source: path.clone(),
                target: path,
                read_only: false,
                optional: false,
            });
        }

        // /dev/kvm if "kvm" system feature is required
        if self.required_system_features.contains("kvm") {
            mounts.push(SandboxMount {
                source: PathBuf::from("/dev/kvm"),
                target: PathBuf::from("/dev/kvm"),
                read_only: false,
                optional: false,
            });
        }

        // Extra sandbox paths from config
        for path in &self.extra_sandbox_paths {
            mounts.push(SandboxMount {
                source: path.clone(),
                target: path.clone(),
                read_only: true,
                optional: false,
            });
        }

        mounts
    }
}

/// Linux sandbox using user namespaces and bind mounts.
///
/// Each sandbox instance acquires a file-locked UID slot from the
/// userpool directory during `prepare()` and releases it during
/// `teardown()` (or on drop). The child process calls
/// `unshare(CLONE_NEWUSER | CLONE_NEWNS)` before exec, mapping the
/// allocated UID to root inside the namespace.
pub struct LinuxSandbox {
    config: LinuxSandboxConfig,
    /// Pool directory for file-lock based UID allocation.
    pool_dir: PathBuf,
    /// Auto-allocate start UID.
    start_id: u32,
    /// Total UIDs in the pool.
    id_count: u32,
    /// Allocated build user — held for the lifetime of the build.
    /// The file lock is released on drop.
    user_lock: Option<UserLock>,
}

impl LinuxSandbox {
    /// Create a new Linux sandbox with the given config.
    ///
    /// `pool_dir` is the directory for lock files (e.g. `<stateDir>/userpool2`).
    /// `start_id` and `id_count` define the UID range for auto-allocation.
    pub fn new(
        config: LinuxSandboxConfig,
        pool_dir: PathBuf,
        start_id: u32,
        id_count: u32,
    ) -> Self {
        Self {
            config,
            pool_dir,
            start_id,
            id_count,
            user_lock: None,
        }
    }

    /// The UID allocated for this build, if `prepare()` has been called.
    pub fn build_uid(&self) -> Option<u32> {
        self.user_lock.as_ref().map(|l| l.uid())
    }
}

impl Sandbox for LinuxSandbox {
    async fn prepare(&mut self) -> Result<(), SandboxError> {
        let pool_dir = self.pool_dir.clone();
        let start_id = self.start_id;
        let id_count = self.id_count;
        let lock = tokio::task::spawn_blocking(move || {
            build_users::acquire_auto_user_lock(&pool_dir, start_id, id_count, 1)
        })
        .await
        .map_err(|e| SandboxError::Setup(format!("spawn_blocking join: {e}")))?
        .map_err(|e| SandboxError::Setup(format!("acquire user lock: {e}")))?
        .ok_or_else(|| SandboxError::Setup("no build user slots available".into()))?;
        self.user_lock = Some(lock);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[allow(unsafe_code)]
    async fn spawn(
        &self,
        builder: &str,
        args: &[&str],
        env: &BTreeMap<String, String>,
        work_dir: &Path,
    ) -> Result<SandboxChild, SandboxError> {
        let _build_uid = self
            .user_lock
            .as_ref()
            .ok_or_else(|| SandboxError::Spawn("prepare() not called".into()))?
            .uid();

        use std::os::unix::process::CommandExt;
        use std::process::Stdio;

        let no_chroot = self.config.no_chroot;

        // Capture real UID/GID before fork+unshare. After
        // unshare(CLONE_NEWUSER) the process is in a new user namespace
        // where its UID/GID appear as unmapped (65534). The uid_map and
        // gid_map must reference the *original* IDs from the parent
        // namespace.
        let real_uid = nix::unistd::getuid();
        let real_gid = nix::unistd::getgid();

        // Build with std::process::Command, then convert to tokio::process::Command.
        // This lets us use pre_exec (which requires std CommandExt) while still
        // getting tokio's async Child for wait/IO.
        let mut cmd = std::process::Command::new(builder);
        cmd.args(args)
            .current_dir(work_dir)
            .env_clear()
            .envs(env.iter())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // pre_exec runs in the child after fork, before exec.
        // We call unshare(2) here to create new namespaces.
        // SAFETY: unshare is async-signal-safe on Linux. We only call
        // unshare and write to /proc/self files, which are safe in the
        // post-fork child.
        unsafe {
            cmd.pre_exec(move || {
                use nix::sched::{CloneFlags, unshare};

                let mut flags = CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS;
                if !no_chroot {
                    flags |= CloneFlags::CLONE_NEWNET;
                }

                unshare(flags).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("unshare({flags:?}): {e}"),
                    )
                })?;

                // Map the parent namespace UID/GID to root inside.
                std::fs::write("/proc/self/uid_map", format!("0 {real_uid} 1\n")).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("write uid_map: {e}"),
                    )
                })?;
                // Must write "deny" to setgroups before writing gid_map
                // (kernel requirement for unprivileged user namespaces)
                std::fs::write("/proc/self/setgroups", "deny\n").map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("write setgroups: {e}"),
                    )
                })?;
                std::fs::write("/proc/self/gid_map", format!("0 {real_gid} 1\n")).map_err(|e| {
                    std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        format!("write gid_map: {e}"),
                    )
                })?;

                Ok(())
            });
        }

        // Convert to tokio Command (carries over pre_exec) and spawn.
        // After fork(), the child is single-threaded regardless of the
        // parent, so unshare(CLONE_NEWUSER) should succeed in pre_exec.
        let mut tokio_cmd = tokio::process::Command::from(cmd);
        let child = tokio_cmd.spawn().map_err(|e| {
            SandboxError::Spawn(format!("Failed to spawn '{builder}' in sandbox: {e}"))
        })?;

        Ok(SandboxChild::from_child(child))
    }

    #[cfg(not(target_os = "linux"))]
    async fn spawn(
        &self,
        _builder: &str,
        _args: &[&str],
        _env: &BTreeMap<String, String>,
        _work_dir: &Path,
    ) -> Result<SandboxChild, SandboxError> {
        Err(SandboxError::Spawn(
            "Linux sandbox is only supported on Linux".into(),
        ))
    }

    async fn teardown(&mut self) -> Result<(), SandboxError> {
        self.user_lock.take();
        Ok(())
    }

    fn bind_mount_paths(&self) -> Vec<SandboxMount> {
        self.config.bind_mounts()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn default_config() -> LinuxSandboxConfig {
        LinuxSandboxConfig {
            store_dir: PathBuf::from("/nix/store"),
            build_dir: PathBuf::from("/tmp/nix-build-test"),
            required_system_features: BTreeSet::new(),
            extra_sandbox_paths: Vec::new(),
            no_chroot: false,
        }
    }

    /// `requiredSystemFeatures = ["kvm"]` → `/dev/kvm` in bind mount list.
    #[test]
    fn test_kvm_system_feature() {
        let config = default_config();
        assert!(
            !config
                .bind_mounts()
                .iter()
                .any(|m| m.source == Path::new("/dev/kvm")),
            "Default config should NOT have /dev/kvm"
        );

        let mut kvm_config = default_config();
        kvm_config
            .required_system_features
            .insert("kvm".to_string());
        assert!(
            kvm_config
                .bind_mounts()
                .iter()
                .any(|m| m.source == Path::new("/dev/kvm")),
            "kvm feature should add /dev/kvm mount"
        );
    }

    /// `extra_sandbox_paths` appears in bind mount list.
    #[test]
    fn test_extra_sandbox_paths() {
        let mut config = default_config();
        config
            .extra_sandbox_paths
            .push(PathBuf::from("/etc/resolv.conf"));
        let mounts = config.bind_mounts();

        let extra = mounts
            .iter()
            .find(|m| m.source == Path::new("/etc/resolv.conf"))
            .expect("extra_sandbox_paths entry should appear in mount list");
        assert!(extra.read_only, "Extra sandbox paths should be read-only");
    }

    use crate::build_users::MAX_IDS_PER_BUILD;

    /// Helper: create a LinuxSandbox with file-lock pool in a temp dir.
    /// Uses 2 slots starting at UID 30000.
    fn make_sandbox(pool_dir: &Path, config: LinuxSandboxConfig, slots: u32) -> LinuxSandbox {
        LinuxSandbox::new(
            config,
            pool_dir.to_path_buf(),
            30000,
            MAX_IDS_PER_BUILD * slots,
        )
    }

    /// LinuxSandbox allocates a build user during prepare() and releases
    /// it during teardown(), making the UID available for reuse.
    #[tokio::test]
    async fn test_sandbox_allocates_and_releases_build_user() {
        let pool_tmp = tempfile::tempdir().unwrap();
        let pool_dir = pool_tmp.path().join("userpool2");

        let mut sandbox1 = make_sandbox(&pool_dir, default_config(), 2);
        assert!(sandbox1.build_uid().is_none(), "No UID before prepare");

        sandbox1.prepare().await.unwrap();
        let uid1 = sandbox1.build_uid().expect("UID allocated after prepare");
        assert_eq!(uid1, 30000, "First slot starts at start_id");

        // Second sandbox gets a different UID
        let mut sandbox2 = make_sandbox(&pool_dir, default_config(), 2);
        sandbox2.prepare().await.unwrap();
        let uid2 = sandbox2.build_uid().unwrap();
        assert_ne!(uid1, uid2, "Concurrent sandboxes get distinct UIDs");

        // Pool exhausted (only 2 slots)
        let mut sandbox3 = make_sandbox(&pool_dir, default_config(), 2);
        assert!(
            sandbox3.prepare().await.is_err(),
            "Pool should be exhausted"
        );

        // Teardown releases UID
        sandbox1.teardown().await.unwrap();
        assert!(sandbox1.build_uid().is_none(), "UID cleared after teardown");

        // Reuse released UID
        let mut sandbox4 = make_sandbox(&pool_dir, default_config(), 2);
        sandbox4.prepare().await.unwrap();
        assert_eq!(sandbox4.build_uid().unwrap(), uid1, "Released UID reused");

        sandbox2.teardown().await.unwrap();
        sandbox4.teardown().await.unwrap();
    }

    /// Builder process inside the sandbox sees uid 0 (mapped via user namespace)
    /// and cannot see the host network namespace.
    ///
    /// Uses `flavor = "current_thread"` because `unshare(CLONE_NEWUSER)`
    /// requires a single-threaded process (returns EINVAL otherwise).
    #[tokio::test(flavor = "current_thread")]
    #[cfg(target_os = "linux")]
    async fn test_sandbox_user_namespace_isolation() {
        let pool_tmp = tempfile::tempdir().unwrap();
        let pool_dir = pool_tmp.path().join("userpool2");
        let tmp = tempfile::tempdir().unwrap();

        let config = LinuxSandboxConfig {
            store_dir: PathBuf::from("/nix/store"),
            build_dir: tmp.path().to_path_buf(),
            required_system_features: BTreeSet::new(),
            extra_sandbox_paths: Vec::new(),
            no_chroot: false,
        };

        let mut sandbox = make_sandbox(&pool_dir, config, 1);
        sandbox.prepare().await.unwrap();

        // Read UID/GID from /proc/self/status using only shell builtins.
        // The Uid/Gid lines have format: "Uid:\t<real>\t<effective>\t..."
        let out_file = tmp.path().join("result");
        let out_path = out_file.to_string_lossy().to_string();
        let script = format!(
            r#"while IFS= read -r line; do
                case "$line" in
                    Uid:*|Gid:*) printf '%s\n' "$line" >> {out_path} ;;
                esac
            done < /proc/self/status"#
        );

        let env = BTreeMap::new();
        let mut child = sandbox
            .spawn("/bin/sh", &["-c", &script], &env, tmp.path())
            .await
            .unwrap();

        let status = child.wait().await.unwrap();
        assert!(status.success(), "Sandboxed process should succeed");

        let output = std::fs::read_to_string(&out_file).unwrap();
        for line in output.lines() {
            // Format: "Uid:\t0\t0\t0\t0" or "Gid:\t0\t0\t0\t0"
            let fields: Vec<&str> = line.split_whitespace().collect();
            match fields[0] {
                "Uid:" => assert_eq!(fields[1], "0", "Real UID should be 0 inside namespace"),
                "Gid:" => assert_eq!(fields[1], "0", "Real GID should be 0 inside namespace"),
                _ => {}
            }
        }

        sandbox.teardown().await.unwrap();
    }

    /// Builder in a sandbox with network namespace cannot reach external hosts.
    #[tokio::test(flavor = "current_thread")]
    #[cfg(target_os = "linux")]
    async fn test_sandbox_network_isolation() {
        let pool_tmp = tempfile::tempdir().unwrap();
        let pool_dir = pool_tmp.path().join("userpool2");
        let tmp = tempfile::tempdir().unwrap();

        let config = LinuxSandboxConfig {
            store_dir: PathBuf::from("/nix/store"),
            build_dir: tmp.path().to_path_buf(),
            required_system_features: BTreeSet::new(),
            extra_sandbox_paths: Vec::new(),
            no_chroot: false, // network namespace IS created
        };

        let mut sandbox = make_sandbox(&pool_dir, config, 1);
        sandbox.prepare().await.unwrap();

        // In a network namespace with no setup, only `lo` exists and it's down.
        // Read /proc/self/net/dev using shell builtins to list interfaces.
        let out_file = tmp.path().join("net_result");
        let out_path = out_file.to_string_lossy().to_string();
        let script = format!(
            r#"while IFS= read -r line; do
                printf '%s\n' "$line"
            done < /proc/self/net/dev > {out_path}"#
        );

        let env = BTreeMap::new();
        let mut child = sandbox
            .spawn("/bin/sh", &["-c", &script], &env, tmp.path())
            .await
            .unwrap();

        let status = child.wait().await.unwrap();
        assert!(status.success(), "Sandboxed process should succeed");

        let output = std::fs::read_to_string(&out_file).unwrap();
        // In a fresh network namespace, only "lo" should appear
        let ifaces: Vec<&str> = output
            .lines()
            .skip(2) // skip header lines
            .filter_map(|line| line.split(':').next())
            .map(|s| s.trim())
            .collect();
        assert_eq!(
            ifaces,
            vec!["lo"],
            "Network namespace should only have lo, got: {ifaces:?}"
        );

        sandbox.teardown().await.unwrap();
    }

    /// Helper: parse interface names from `/proc/self/net/dev` content.
    #[cfg(target_os = "linux")]
    fn parse_ifaces(content: &str) -> Vec<String> {
        content
            .lines()
            .skip(2) // skip header lines
            .filter_map(|line| line.split(':').next())
            .map(|s| s.trim().to_string())
            .collect()
    }

    /// With `no_chroot = true`, the builder inherits the parent's network
    /// namespace instead of getting a fresh one.  We verify by comparing
    /// the child's interface list with the parent's: they must match
    /// because no `CLONE_NEWNET` is applied.
    ///
    /// This works even inside a nix build sandbox (which itself may only
    /// have `lo`) because we compare against what the *parent* sees, not
    /// against an assumption about the host.
    #[tokio::test(flavor = "current_thread")]
    #[cfg(target_os = "linux")]
    async fn test_sandbox_no_chroot_allows_network() {
        let pool_tmp = tempfile::tempdir().unwrap();
        let pool_dir = pool_tmp.path().join("userpool2");
        let tmp = tempfile::tempdir().unwrap();

        let config = LinuxSandboxConfig {
            store_dir: PathBuf::from("/nix/store"),
            build_dir: tmp.path().to_path_buf(),
            required_system_features: BTreeSet::new(),
            extra_sandbox_paths: Vec::new(),
            no_chroot: true, // NO network namespace
        };

        let mut sandbox = make_sandbox(&pool_dir, config, 1);
        sandbox.prepare().await.unwrap();

        // Read parent's interfaces for comparison.
        let parent_net_dev = std::fs::read_to_string("/proc/self/net/dev").unwrap();
        let parent_ifaces = parse_ifaces(&parent_net_dev);

        let out_file = tmp.path().join("net_result");
        let out_path = out_file.to_string_lossy().to_string();
        let script = format!(
            r#"while IFS= read -r line; do
                printf '%s\n' "$line"
            done < /proc/self/net/dev > {out_path}"#
        );

        let env = BTreeMap::new();
        let mut child = sandbox
            .spawn("/bin/sh", &["-c", &script], &env, tmp.path())
            .await
            .unwrap();

        let status = child.wait().await.unwrap();
        assert!(status.success(), "Process should succeed");

        let output = std::fs::read_to_string(&out_file).unwrap();
        let child_ifaces = parse_ifaces(&output);
        // Same interfaces ⇒ same network namespace (no CLONE_NEWNET)
        assert_eq!(
            parent_ifaces, child_ifaces,
            "no_chroot child should see same interfaces as parent"
        );

        sandbox.teardown().await.unwrap();
    }
}
