// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Sandbox abstraction for build isolation.
//!
//! Defines the `Sandbox` trait with platform implementations:
//! - `NoSandbox`: passthrough for `sandbox = false` config
//! - Future: `LinuxSandbox` (user namespaces), `DarwinSandbox` (sandbox-exec)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// A mount entry in the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxMount {
    /// Source path on the host.
    pub source: PathBuf,
    /// Destination path in the sandbox.
    pub target: PathBuf,
    /// Whether the mount is read-only.
    pub read_only: bool,
    /// Whether the mount is optional (missing source is tolerated).
    pub optional: bool,
}

/// Errors from sandbox operations.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("sandbox setup failed: {0}")]
    Setup(String),
    #[error("sandbox spawn failed: {0}")]
    Spawn(String),
    #[error("sandbox teardown failed: {0}")]
    Teardown(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Sandbox environment prepared for a build.
///
/// The sandbox controls how the builder process is spawned and isolated.
/// Implementations handle namespace setup (Linux), sandbox profiles (macOS),
/// or direct execution (NoSandbox).
pub trait Sandbox: Send + Sync {
    /// Prepare the sandbox environment before spawning the builder.
    ///
    /// This may create namespaces, set up bind mounts, write sandbox
    /// profiles, allocate build users, etc.
    fn prepare(&mut self) -> impl std::future::Future<Output = Result<(), SandboxError>> + Send;

    /// Spawn the builder process within the sandbox.
    ///
    /// Returns a handle that can be used to wait for completion and
    /// read log output.
    fn spawn(
        &self,
        builder: &str,
        args: &[&str],
        env: &BTreeMap<String, String>,
        work_dir: &Path,
    ) -> impl std::future::Future<Output = Result<SandboxChild, SandboxError>> + Send;

    /// Tear down the sandbox after the build completes.
    ///
    /// Cleans up namespaces, releases build users, removes temporary
    /// mounts, etc.
    fn teardown(&mut self) -> impl std::future::Future<Output = Result<(), SandboxError>> + Send;

    /// List of paths that will be bind-mounted into the sandbox (for testing).
    fn bind_mount_paths(&self) -> Vec<SandboxMount> {
        Vec::new()
    }
}

/// A running builder process inside a sandbox.
pub struct SandboxChild {
    inner: tokio::process::Child,
}

impl SandboxChild {
    /// Wrap a tokio child process as a `SandboxChild`.
    pub fn from_child(child: tokio::process::Child) -> Self {
        Self { inner: child }
    }

    /// Wait for the process to exit and return its status.
    pub async fn wait(&mut self) -> Result<ExitStatus, SandboxError> {
        self.inner.wait().await.map_err(SandboxError::Io)
    }

    /// Kill the process.
    pub async fn kill(&mut self) -> Result<(), SandboxError> {
        self.inner.kill().await.map_err(SandboxError::Io)
    }

    /// Take stdout for reading (can only be called once).
    pub fn take_stdout(&mut self) -> Option<impl tokio::io::AsyncRead + Send + Unpin + '_> {
        self.inner.stdout.take()
    }

    /// Take stderr for reading (can only be called once).
    pub fn take_stderr(&mut self) -> Option<impl tokio::io::AsyncRead + Send + Unpin + '_> {
        self.inner.stderr.take()
    }

    /// Get the process ID (for process group kill).
    pub fn pid(&self) -> Option<u32> {
        self.inner.id()
    }
}

/// No-sandbox passthrough implementation.
///
/// The builder runs directly as a child process without any isolation.
/// Used when `sandbox = false` in the daemon config.
pub struct NoSandbox;

impl Default for NoSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl NoSandbox {
    pub fn new() -> Self {
        NoSandbox
    }
}

impl Sandbox for NoSandbox {
    async fn prepare(&mut self) -> Result<(), SandboxError> {
        // Nothing to prepare — no isolation
        Ok(())
    }

    async fn spawn(
        &self,
        builder: &str,
        args: &[&str],
        env: &BTreeMap<String, String>,
        work_dir: &Path,
    ) -> Result<SandboxChild, SandboxError> {
        use std::process::Stdio;

        let mut cmd = tokio::process::Command::new(builder);
        cmd.args(args)
            .current_dir(work_dir)
            .env_clear()
            .envs(env.iter())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| SandboxError::Spawn(format!("Failed to spawn '{builder}': {e}")))?;

        Ok(SandboxChild::from_child(child))
    }

    async fn teardown(&mut self) -> Result<(), SandboxError> {
        // Nothing to tear down
        Ok(())
    }
}
