use nix::unistd::{Gid, Uid};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{DaemonError, IoContext};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Path to bind the daemon socket
    pub socket_path: PathBuf,

    /// Path to the Nix store directory
    pub store_dir: PathBuf,

    /// Path to the Nix database
    pub db_path: PathBuf,

    /// Number of worker threads
    pub workers: Option<usize>,

    /// Log level
    pub log_level: String,

    /// Enable build sandbox.
    ///
    /// On Linux this uses user namespaces; when root, build UIDs are
    /// auto-allocated via file locks.  On macOS this requires
    /// `build_users_group` to be set.
    pub sandbox: bool,

    /// Name of the Unix group whose members serve as build users.
    ///
    /// Only used on macOS.  On Linux, UIDs are auto-allocated and this
    /// field is ignored.  Matches Nix's `build-users-group` setting.
    pub build_users_group: Option<String>,

    /// Directory for build-user file locks.
    ///
    /// Each build user slot is represented by a lock file in this
    /// directory, matching Nix's userpool layout.
    pub pool_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/run/harmonia-daemon.sock"),
            store_dir: PathBuf::from("/nix/store"),
            db_path: PathBuf::from("/nix/var/nix/db/db.sqlite"),
            workers: None,
            log_level: "info".to_string(),
            sandbox: true,
            build_users_group: None,
            pool_dir: PathBuf::from("/nix/var/nix/userpool"),
        }
    }
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self, DaemonError> {
        let contents = std::fs::read_to_string(path)
            .io_context(|| format!("Failed to read config file at {}", path.display()))?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }
}

/// How the daemon isolates builder processes.
///
/// On Linux the sandbox always uses user namespaces (`CLONE_NEWUSER`).
/// When the daemon runs as root it auto-allocates build UIDs via file
/// locks; when unprivileged it maps the daemon's own UID into the
/// sandbox (UID 1000 inside, matching Nix).
///
/// On macOS the sandbox requires a `build_users_group` whose members
/// are used as build users (matching Nix's `build-users-group`).
#[derive(Debug, Clone, Default)]
pub enum SandboxConfig {
    /// No isolation — builder runs as the daemon's own user.
    #[default]
    Off,
    /// Sandbox enabled.
    On {
        /// Directory for build-user file locks (used on Linux when root).
        pool_dir: PathBuf,
        /// Resolved `(Uid, Gid)` pairs from the build-users-group.
        /// Required on macOS; unused on Linux (auto-allocate instead).
        group_members: Vec<(Uid, Gid)>,
    },
}

impl SandboxConfig {
    /// Construct a sandbox config for Linux.
    ///
    /// On Linux we always use auto-allocated UIDs + user namespaces,
    /// so no group resolution is needed.
    pub fn new_linux(pool_dir: PathBuf) -> Self {
        SandboxConfig::On {
            pool_dir,
            group_members: Vec::new(),
        }
    }

    /// Construct a sandbox config for macOS by resolving the given
    /// Unix group name to its `(uid, gid)` member list.
    ///
    /// Returns an error if the group doesn't exist or has no members.
    pub fn from_group_name(pool_dir: PathBuf, group_name: &str) -> Result<Self, String> {
        let group = nix::unistd::Group::from_name(group_name)
            .map_err(|e| format!("failed to look up group '{group_name}': {e}"))?
            .ok_or_else(|| format!("build-users-group '{group_name}' does not exist"))?;

        let gid = group.gid;
        let mut members = Vec::new();
        for username in &group.mem {
            let user = nix::unistd::User::from_name(username.as_str())
                .map_err(|e| format!("failed to look up user '{username}': {e}"))?
                .ok_or_else(|| {
                    format!("user '{username}' in group '{group_name}' does not exist")
                })?;
            members.push((user.uid, gid));
        }

        if members.is_empty() {
            return Err(format!("build-users-group '{group_name}' has no members"));
        }

        Ok(SandboxConfig::On {
            pool_dir,
            group_members: members,
        })
    }
}
