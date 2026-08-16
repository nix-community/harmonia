# Incremental Delivery Plan

The architecture is designed for multi-coordinator HA from day 1 —
`FOR UPDATE SKIP LOCKED`, node registration, and heartbeat reaping are
not separate phases but part of the core design. The phases below
represent feature milestones.

## Phase 1: harmonia-daemon Build Executor

Extend harmonia-daemon from a read-only store into a full build executor
usable both standalone (nix-daemon replacement) and as the backend for
the CI builder role. See [harmonia-daemon.md](harmonia-daemon.md).

- Read-write SQLite database mode
- `add_to_store_nar` / `add_multiple_to_store` (store writes)
- `nar_from_path` (NAR serialization)
- `build_derivation` (exec, log streaming, output registration)
- `query_missing` (substitution planning)
- Linux sandbox (user namespaces, bind mounts, chroot)
- macOS sandbox (`sandbox-exec`)
- Internal build scheduling (`max_jobs`, DAG execution, substitution)

## Phase 2: Single-Project CI

First end-to-end CI pipeline, single project, CLI-triggered.

- `harmonia-ci` binary with all four capabilities
- PostgreSQL schema (Layer 1 + Layer 2) with sqlx migrations
- nix-eval-jobs integration: eval → derivations → build_jobs
  - Batched .drv uploads (presigned URLs from signer)
  - Eval timeout (global default, configurable)
  - Evaluator sandboxing (nix-eval-jobs subprocess has no access to secrets)
  - IFD allowed by default, opt-out via `.harmonia-ci.toml`
- Work-stealing build dispatch with DAG-aware readiness
- Shares-based fair scheduling between projects
- Dependency failure propagation (`dep-failed` status, recursive CTE)
- Presigned S3 upload, signer signs narinfo, signer failover
- Frankenbuild prevention (input validation via local Nix SQLite)
- Build log streaming (UNLOGGED `build_log_tails` + S3 archive)
- Build timeouts: wall-clock (4h) + max-silent (30m)
- Auto-retry (3×), GC root management on builders
- Serve build status at `/ci/` HTTP endpoint
- CLI eval trigger: `harmonia-ci eval <flake-url>` (webhooks are Phase 3)
- Single project config (TOML)

## Phase 3: Forge Integration
- Forge connection config (GitHub App, Gitea token)
- Repository discovery (sync from forge APIs into `repositories` table)
- Project onboarding UI (searchable list, enable/disable toggle, settings)
- Webhook auto-registration (Gitea) and handler (push + pull_request)
- HMAC signature verification (GitHub, Gitea)
- Webhook idempotency (`UNIQUE(project_id, commit_sha, branch)`)
- PR trust levels (collaborators / all / none)
- Commit status reporter (nix-eval + nix-build per eval)
  - Eval completion detection via LISTEN + aggregate query
- Eval superseding (new push cancels old eval)
- Tree hash dedup (skip eval if same tree hash already succeeded)
- Polling fallback (per-project `poll_interval`, default disabled)
- Multi-project support with per-project priority config

## Phase 4: Web UI
- HTMX-based dashboard (project list, eval list, build queue)
- Live build log viewer (SSE-backed)
- Per-attr build history
- Link from forge commit status → Harmonia build log
- Manual rebuild: single build, eval rebuild-failed, re-evaluate
- Manual cancel: eval cancel, build cancel
- Node management: fleet status, draining via API
- Auth: forge OAuth, signed cookies, permission mirroring

## Phase 5: Observability
- Prometheus metrics endpoint:
  - eval duration, build queue depth, builds in progress
  - retry rate, failure rate by project
  - node utilisation (jobs / max_jobs)
  - per-project share consumption
- Alerting recommendations (stale nodes, queue backup)

## Phase 6 (Optional): PostgreSQL Eval Store Plugin
- C++ Nix store plugin (`harmonia-store-pg`)
- `--eval-store pg://…` support in nix-eval-jobs
- Eliminates JSON round-trip; derivations written directly to DB
