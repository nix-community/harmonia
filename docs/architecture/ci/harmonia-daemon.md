# harmonia-daemon: Build Executor

## Overview

harmonia-daemon is the local build executor. It speaks the Nix daemon
Unix socket protocol, receives `BuildDerivation` requests, spawns
builder processes in a sandbox, streams log output back over the
protocol, and registers outputs in the local Nix SQLite database.

It is designed to work in two modes:

1. **CI mode** — driven by the `harmonia-ci` builder role, which
   handles job claiming, GC roots, Frankenbuild validation, log
   persistence, and S3 upload. The daemon is a pure executor.

2. **Standalone mode** — used as a drop-in replacement for nix-daemon.
   Clients (e.g. `nix build`, `nix-store`) connect directly. The
   daemon handles its own internal build scheduling: concurrent build
   limits, build queue management, and resource accounting.

Both modes use the same protocol and the same build machinery. The
difference is who manages scheduling — in CI mode, the builder role
sends one `BuildDerivation` at a time; in standalone mode, the daemon
manages its own queue internally.

```
harmonia-ci (builder role)           harmonia-daemon
┌──────────────────────────┐        ┌──────────────────────────┐
│ Claims job from PG       │        │                          │
│ Validates inputs (Frank.)│        │                          │
│ Creates GC roots         │        │                          │
│                          │  Unix  │                          │
│ AddToStoreNar (subst.)  ─┼──sock──┼─► Write NAR to store    │
│ BuildDerivation          ─┼──────►┼─► Sandbox + exec builder │
│   ◄── log stream ────────┼────────┼── stdout/stderr capture  │
│ NarFromPath (read out)  ─┼───────►┼─► Serialize to NAR       │
│                          │        │                          │
│ Buffers full log         │        │ Registers outputs in     │
│ Pushes tail to PG        │        │ local SQLite DB          │
│ Uploads NAR to S3        │        │                          │
│ Reports to signer        │        │                          │
└──────────────────────────┘        └──────────────────────────┘
```

This split keeps harmonia-daemon free of any CI concerns (no PostgreSQL,
no S3, no scheduling, no signing). It is a local Nix store + build
executor with a standard daemon protocol interface.

## Current State

harmonia-daemon today is a **read-only** store daemon. It implements:

- `is_valid_path` — check if a store path exists
- `query_path_info` — return metadata for a store path
- `query_path_from_hash_part` — look up a path by hash prefix
- `query_valid_paths` — batch validity check

The server already dispatches `BuildDerivation`, `AddToStoreNar`,
`AddMultipleToStore`, `BuildPathsWithResults`, `NarFromPath`, and
`QueryMissing` to the `DaemonStore` trait — but all return
`unimplemented`. The database is opened in `ReadOnly` mode.

## Required Changes

### 1. Read-Write Database Mode

**Scope**: `LocalStoreHandler::new`, `Config`

Open the Nix SQLite database in read-write mode so that substituted
inputs and build outputs can be registered. Add a config flag:

```toml
[daemon]
read_only = false  # default: false (read-write for builders)
```

harmonia-cache deployments that only serve the store can keep
`read_only = true`.

### 2. `add_to_store_nar`

**Scope**: `LocalStoreHandler`, `harmonia-store-db`

The builder substitutes inputs from the S3 binary cache. Substituted
paths arrive as NAR streams via `AddToStoreNar`:

1. Receive the NAR stream from the protocol connection
2. Unpack the NAR into `/nix/store/<path>`
3. Compute and verify the NAR hash (SHA-256)
4. Verify signatures against trusted public keys (unless
   `dont_check_sigs`)
5. Register the path in the SQLite `ValidPaths` table (hash,
   references, deriver, signatures, registration time)

This also requires extending `harmonia-store-db` with write operations:
`register_valid_path`, `add_references`.

### 3. `add_multiple_to_store`

**Scope**: `LocalStoreHandler`

Batch variant of `add_to_store_nar`. The protocol already parses the
framed stream into individual `AddToStoreItem`s — the handler iterates
and delegates to the same write logic.

### 4. `nar_from_path`

**Scope**: `LocalStoreHandler`, `harmonia-nar`

After a build completes, the builder reads outputs back to compute the
NAR hash and zstd-compress for S3 upload:

1. Receive a store path
2. Serialize the store path contents to NAR format (using
   `harmonia-nar`)
3. Stream the NAR data back over the protocol connection

This is a read-only operation on the filesystem but requires NAR
serialization (directory traversal in sorted order, file content
streaming).

### 5. `build_derivation`

**Scope**: `LocalStoreHandler`, new `build` module

This is the core change. When the builder sends `BuildDerivation`:

1. **Parse the `BasicDerivation`**: extract builder path, args,
   environment variables, input paths, output paths, and
   `requiredSystemFeatures`.

2. **Validate inputs**: all input store paths must exist locally
   (the builder is responsible for substituting them first).

3. **Prepare the build environment**:
   - Create a temporary build directory
   - Set up environment variables from the derivation (`env` map)
   - Set standard Nix build variables (`NIX_BUILD_TOP`, `NIX_STORE`,
     `out`, `outputs`, etc.)

4. **Sandbox the build** (platform-specific):
   - **Linux**: user namespaces, bind mounts (`/nix/store` read-only,
     build dir read-write, `/proc`, `/dev/null`, `/dev/zero`,
     `/dev/urandom`), private network namespace (unless
     `__noChroot = true`), chroot
   - **macOS**: `sandbox-exec` with a restrictive profile

5. **Execute the builder**: spawn the process, capture stdout and
   stderr.

6. **Stream logs**: forward builder stdout/stderr as `LogMessage`
   frames over the daemon protocol. The builder binary on the other
   end handles log buffering, PG tail updates, and timeout detection
   (wall-clock and max-silent).

7. **On success**:
   - Verify output paths exist
   - Compute NAR hash for each output
   - Register outputs in the SQLite `ValidPaths` table
   - Return `BuildResult` with `BuildStatus::Built` and output hashes

8. **On failure**:
   - Return `BuildResult` with `BuildStatus::MiscFailure` or
     `BuildStatus::OutputRejected` and the exit code
   - Clean up partial outputs

### 6. `query_missing`

**Scope**: `LocalStoreHandler`

Given a set of `DerivedPath`, determine which paths need building vs
which are already in the store. Used by the builder to plan substitution
before building:

1. For `DerivedPath::Opaque` — check if the path exists in SQLite
2. For `DerivedPath::Built` — check if the derivation's outputs exist

Return the sets: `will_build`, `will_substitute`, `unknown`, and
estimated download/NAR sizes.

## Internal Build Scheduling (Standalone Mode)

When used outside the CI system — as a drop-in replacement for
nix-daemon — harmonia-daemon must handle its own build scheduling.
Multiple clients may connect simultaneously (e.g. several `nix build`
invocations), and a single `BuildPaths` request may require building
an entire dependency graph.

### Concurrent Build Limits

The daemon enforces a maximum number of concurrent builds:

```toml
[build]
max_jobs = 4        # max concurrent builds (0 = auto-detect from CPU count)
cores = 0           # cores per build (0 = all cores available to the build)
```

When all build slots are occupied, new `BuildDerivation` requests queue
internally until a slot becomes available. The queue is FIFO.

`max_jobs` and `cores` mirror nix-daemon's `--max-jobs` and `--cores`
semantics so existing configurations transfer directly.

### Build Queue and DAG Execution

When a client sends `BuildPaths` with a derivation that has
dependencies, the daemon must:

1. Resolve the full dependency DAG from the derivation inputs
2. Topologically sort: leaf derivations build first
3. As builds complete, unblock dependents (same wavefront approach
   as the CI scheduler, but local)
4. Substitute available paths from configured substituters rather
   than building them

The internal scheduler is simpler than the CI scheduler — no
inter-project fairness, no PostgreSQL, no shares. It is a local
DAG executor with a bounded thread pool.

### Resource Accounting

The daemon tracks per-build resource usage locally:

- **Build slots**: semaphore of size `max_jobs`
- **Disk space**: check available space before starting a build;
  fail early with a clear error rather than mid-build ENOSPC
- **Memory**: no hard limit (relies on OS OOM killer), but log
  peak RSS per build for diagnostics

### Substitution

In standalone mode the daemon handles substitution directly. When
a required input path is missing locally, the daemon fetches it from
configured substituters (`/etc/nix/nix.conf` `substituters` list)
before starting the build. This mirrors nix-daemon behaviour.

In CI mode, the builder role handles substitution via `AddToStoreNar`
before sending `BuildDerivation`, so the daemon does not substitute
on its own.

### Configuration

```toml
[build]
max_jobs = 4
cores = 0
sandbox = true          # enable sandboxing (default: true)
timeout = "4h"          # wall-clock timeout per build
max_silent = "30m"      # kill build if no output for this long
extra_sandbox_paths = []  # additional paths to bind-mount into sandbox
fallback = false        # build locally if substitution fails

[substituters]
urls = ["https://cache.nixos.org"]
trusted_public_keys = ["cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="]
```

## What Does NOT Change

- **No PostgreSQL awareness** — the daemon knows only the local Nix
  SQLite store
- **No S3 awareness** — output upload is the builder's job
- **No scheduling** — the daemon builds exactly what it's told
- **No signing** — the signer role handles Ed25519 signatures
- **No log storage** — the daemon streams logs over the protocol; the
  builder handles buffering and persistence

## Implementation Order

| Phase | Change | Depends on |
|-------|--------|------------|
| 1 | Read-write DB mode | — |
| 2 | `add_to_store_nar` + `harmonia-store-db` write ops | Phase 1 |
| 3 | `add_multiple_to_store` | Phase 2 |
| 4 | `nar_from_path` | — |
| 5 | `build_derivation` (basic, no sandbox) | Phase 1, 2 |
| 6 | `build_derivation` (Linux sandbox) | Phase 5 |
| 7 | `build_derivation` (macOS sandbox) | Phase 5 |
| 8 | `query_missing` | — |
| 9 | Internal build scheduling (`build_paths` DAG execution, `max_jobs` semaphore, substitution) | Phase 5 |

Phases 1–5 are sufficient for a working (unsandboxed) CI builder.
Sandboxing (phases 6–7) is required for production use with untrusted
inputs (PR builds). Phase 9 enables standalone use as a nix-daemon
replacement.

## Testing Strategy

- **Unit tests**: each `DaemonStore` method tested against a
  Nix-initialized temporary store (existing pattern in
  `tests/sqlite_nix_store.rs`)
- **Integration tests**: full round-trip — substitute inputs via
  `add_to_store_nar`, build a trivial derivation via
  `build_derivation`, read output via `nar_from_path`
- **Protocol compatibility**: verify harmonia-daemon accepts requests
  from a standard `nix` client (`nix build --store unix:///path/to/sock`)
