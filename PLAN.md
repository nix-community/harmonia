# Migration Plan: Wire Sandbox into Build Pipeline

## Goal

Make harmonia-daemon use the `LinuxSandbox` (privileged mode) for builds,
so builders run as dedicated build users with correct supplementary GIDs.
Verify end-to-end with a NixOS VM test that triggers a real build through
the daemon socket and checks the builder's credentials.

## Commit 1: Refactor build spawning to use `SandboxChild` (pure refactor)

No behavior change. All 82 existing tests pass unchanged.

### sandbox.rs

- `take_stdout()` → return `Option<tokio::process::ChildStdout>` (concrete, owned, movable into spawned tasks)
- `take_stderr()` → return `Option<tokio::process::ChildStderr>` (concrete, owned)
- `NoSandbox::spawn()` → add `.process_group(0)` so timeout kills work the same as today's `execute_builder`

### build.rs

- Extract `monitor_child(child: SandboxChild, config: &BuildConfig, log_sink: &Arc<Mutex<dyn Write + Send>>) → Result<(), BuildError>` from the second half of `execute_builder` — contains all stdout/stderr draining + timeout + process-group-kill logic.
- `execute_builder` becomes: `NoSandbox.spawn()` → `monitor_child()`.
- No signature changes to `build_derivation` or callers.

## Commit 2: Wire sandbox into build pipeline via `SandboxConfig` enum

### config.rs — add `SandboxConfig`

```rust
/// How the daemon isolates builder processes.
#[derive(Debug, Clone, Default)]
pub enum SandboxConfig {
    /// No isolation — builder runs as the daemon's own user.
    #[default]
    None,
    /// Privileged mode (requires root): drop to a build user from
    /// `build_users_group` via setgroups/setgid/setuid.
    Privileged {
        pool_dir: PathBuf,
        build_users_group: String,
    },
}
```

### build.rs

- `build_derivation()` gains a `sandbox_config: &SandboxConfig` parameter.
- Before spawning, creates the appropriate sandbox:
  - `SandboxConfig::None` → `NoSandbox`
  - `SandboxConfig::Privileged { .. }` → `LinuxSandbox::new_privileged(..)`
    (resolve group members via `nix::unistd::Group::from_name`)
- Calls `sandbox.prepare()`, uses `sandbox.spawn()` + `monitor_child()`,
  then `sandbox.teardown()` (also on error paths).
- Remove standalone `execute_builder` — its logic is now:
  sandbox.spawn() → monitor_child().

### handler.rs

- `LocalStoreHandler` stores a `SandboxConfig` field.
- `new()` / `from_shared_db()` default to `SandboxConfig::None`.
- Add `set_sandbox_config(&mut self, config: SandboxConfig)` setter.
- `build_derivation` impl passes `&self.sandbox_config` to `build::build_derivation()`.

### main.rs

- Read `build_users_group` from `Config`.
- If set, construct `SandboxConfig::Privileged { pool_dir, build_users_group }`.
- Call `handler.set_sandbox_config(sandbox_config)`.

### config.rs (daemon config)

- Add optional `build_users_group: Option<String>` field to `Config`.
- Add `pool_dir` field (default `/nix/var/nix/userpool`).

### module.nix

- Add `daemon.buildUsersGroup` option (default `null`).
- When set, pass `build_users_group` in the daemon TOML config.
- When set, add `ReadWritePaths` for the pool directory.
- When set, ensure the service runs as root (it already does).

## Commit 3: NixOS VM test exercising privileged builds

### tests/privileged-sandbox.nix

- Set up 4 nixbld users in group `nixbld`.
- nixbld1 gets supplementary group `testgrp` (gid 1234).
- Configure harmonia-daemon with `build_users_group = "nixbld"`.
- Disable the stock nix-daemon to avoid socket conflicts.
- Write a trivial derivation whose builder is `/bin/sh -c "id > $out"`.
- Build it via `nix-store --store unix:///run/harmonia-daemon/socket --realise <drv>`.
- Read the output and assert:
  - UID is one of the nixbld users (not 0).
  - GID is the nixbld group.
  - If the allocated user is nixbld1, supplementary groups include `testgrp` (1234).
- Check daemon logs for errors.

### flake.nix

- Already wired: `privileged-sandbox = import ./tests/privileged-sandbox.nix testArgs;`
