# Harmonia CI

**Date**: 2026-02-21

## Goal

Build a Nix-native CI system inside Harmonia that can replace
[buildbot-nix](https://github.com/nix-community/buildbot-nix). The core
insight is that Harmonia already owns the binary cache and the build
executor — harmonia-daemon builds derivations directly (no external
nix-daemon required), and outputs are uploaded straight to S3, making
build results instantly available in the cache.

## Architecture Overview

There is one binary: `harmonia-ci`. Every node in the cluster runs the
same binary. Roles are configured per node, not baked into separate
binaries.

```
┌─────────────────────────────────────────────────────────────────────┐
│  harmonia-ci  (one binary, capability flags per node)               │
│                                                                     │
│  capability: frontend                                               │
│    - Webhook receiver (GitHub / Gitea)                              │
│    - Inserts evaluations rows, fires NOTIFY                         │
│    - Listens for completion, reports commit status to forge         │
│    - Serves web UI (HTMX, minimal)                                  │
│    - Does NOT run nix-eval-jobs or nix build                        │
│                                                                     │
│  capability: evaluator                                              │
│    - Polls PostgreSQL for pending evaluations (FOR UPDATE SKIP LOCKED)│
│    - Fetches flake source (git / nix flake fetch)                   │
│    - Runs nix-eval-jobs, streams JSON output into PostgreSQL        │
│    - Requires: nix-eval-jobs binary, local Nix store, RAM (~4 GB)  │
│    - Does NOT need large disk or S3 credentials                     │
│                                                                     │
│  capability: signer                                                 │
│    - Holds Ed25519 signing key and S3 credentials                   │
│    - Issues presigned S3 PUT URLs to builders                       │
│    - Signs and writes narinfo to S3 after build confirmation        │
│                                                                     │
│  capability: builder                                                │
│    - Polls PostgreSQL for pending build_jobs (FOR UPDATE SKIP LOCKED)│
│    - Substitutes inputs from S3/binary cache                        │
│    - Builds via harmonia-daemon (extends existing daemon with       │
│      BuildDerivation support — no external nix-daemon needed)       │
│    - Computes NAR hash + zstd-compresses output                     │
│    - Calls signer endpoint, PUT NARs directly to S3                 │
│    - Requires: large Nix store, build cores                         │
└─────────────────────────────────────────────────────────────────────┘
              │ all nodes share one PostgreSQL instance
              ▼
┌──────────────────────────────────────────────────────────────┐
│  PostgreSQL                                                  │
│  Layer 1: Nix store schema (derivations, inputs, outputs)    │
│  Layer 2: CI schema (evaluations, eval_attrs, build_jobs)            │
└──────────────────────────────────────────────────────────────┘
              │                         │
              │ NARs + narinfo          │ narinfo served as
              ▼                         ▼ Nix binary cache
┌─────────────────────┐    ┌────────────────────────┐
│  S3 / object store  │◄───│  harmonia-cache         │
│  *.narinfo          │    │  (in front of S3,       │
│  nar/*.nar.zst      │    │   optional CDN layer)   │
│  log/*.drv          │    │                         │
└─────────────────────┘    └────────────────────────┘
```

**Signer discovery**: Builders discover available signers by querying the
`nodes` table for rows where `capabilities @> '{signer}'` and `last_seen`
is recent (see [high-availability.md](high-availability.md#node-registration)).

### Typical deployments

**Single machine (dev / small team)**: all four capabilities on one node.
PostgreSQL local. S3 can be a local MinIO instance.

**Small cluster**: a few nodes each running `frontend + evaluator + signer
+ builder`. Every node can do everything. PostgreSQL on a managed instance.

**Large cluster**: specialised by hardware profile:
- 1–2 `frontend + signer` nodes: low traffic, hold the key, no Nix store
  needed beyond what the OS provides
- N `evaluator` nodes: high RAM (4–8 GB per parallel eval), fast network
  to fetch flake inputs, modest disk — no large store needed
- M `builder` nodes: many cores, large NVMe store, no signing key material

Capabilities are additive: a beefy builder can also run `evaluator` if
spare RAM is available. There is no required topology.

## Documents

| Document | Contents |
|---|---|
| [database.md](database.md) | Schema design (Layer 1: Nix store, Layer 2: CI), entity relationships |
| [evaluation.md](evaluation.md) | Eval flow, nix-eval-jobs integration, eval claiming |
| [build-protocol.md](build-protocol.md) | Scheduling, work-stealing, retries, timeouts, GC, Frankenbuild prevention, log streaming |
| [signing-and-upload.md](signing-and-upload.md) | Presigned S3 URLs, signer role, compression, S3 layout |
| [high-availability.md](high-availability.md) | Multi-master, job claiming, heartbeat, LISTEN/NOTIFY |
| [forge-integration.md](forge-integration.md) | Webhooks, PR security, status reporting, project config, supported forges |
| [harmonia-daemon.md](harmonia-daemon.md) | Daemon changes for build execution: protocol methods, sandbox, store writes |
| [webui.md](webui.md) | HTMX web UI: dashboard, project/eval/build pages, live log streaming |
| [gc-and-retention.md](gc-and-retention.md) | Binary cache GC (mark-sweep from eval roots), retention policy, in-tree config |
| [security.md](security.md) | Trust zones, threat analysis, secret material, sandbox requirements |
| [delivery-plan.md](delivery-plan.md) | Phased rollout |

## Key Design Decisions

1. **PostgreSQL, not SQLite** — multi-coordinator HA requires a shared
   database. SQLite WAL mode supports one writer; PostgreSQL with
   `FOR UPDATE SKIP LOCKED` supports N concurrent coordinators cleanly.

2. **No pgmq** — the job queue is a plain `build_jobs` table with typed
   columns (`system`, `drv_path`, `status`). `FOR UPDATE SKIP LOCKED` is
   the entire claiming primitive. pgmq's JSONB blob would make
   system-based routing and metrics harder.

3. **No WAMP/message broker** — PostgreSQL `LISTEN/NOTIFY` is sufficient
   for waking workers. All coordinator state lives in the database, making
   coordinators stateless and the broker unnecessary.

4. **Multi-arch routing via claim query** — each builder node registers
   its supported `systems` in PostgreSQL and only claims `build_jobs`
   rows where `d.system = ANY(node.systems)`. No central scheduler,
   no `nix.conf` machines list, no SSH routing. A builder that cannot
   run `aarch64-linux` simply never picks up those jobs.

5. **Two schema layers, strict separation** — Layer 1 (Nix store schema)
   knows nothing about CI. Layer 2 (CI schema) knows nothing about NAR
   hashes or derivation serialisation. They join only on `drv_path`.

6. **cacheStatus is ephemeral** — `neededBuilds`, `neededSubstitutes`, and
   `cacheStatus` from nix-eval-jobs are used only to decide whether to
   insert a `build_jobs` row. They are never stored.

7. **Deduplication via UNIQUE(drv_path)** — two evaluations that produce
   the same `drvPath` share one `build_jobs` row. No duplicate builds.

8. **harmonia-cache is the binary cache** — workers build on machines that
   also run harmonia-cache. Results are instantly available without a
   push step.

9. **DAG-aware scheduling with shares-based fairness** — jobs are
   claimable only when all input derivations are satisfied (in cache or
   `succeeded`). Among ready jobs: manual-boost builds go first
   (absolute), then shares-based fairness between projects
   (`consumed_seconds / shares`, lowest first), then FIFO. Branch
   priority boosts were considered unnecessary — within a project,
   FIFO is sufficient. LPT was evaluated via simulation and rejected
   (~1-2% makespan improvement doesn't justify the complexity).

10. **Eval superseding** — a new push to the same project+branch cancels
    any queued or running evaluation for that branch. Pending builds
    exclusively owned by cancelled evals are dropped; builds already
    running finish (their outputs are useful in the cache regardless).

11. **No build-time secrets** — builds are pure Nix derivations. Secret
    support (vault integration, sandbox-mounted credentials) is deferred
    to a later phase.

12. **Skip upstream-cached paths** — if a derivation's outputs are
    already available in a configured substituter (e.g. cache.nixos.org),
    no `build_jobs` row is inserted. The evaluator's `cacheStatus` check
    handles this.

13. **requiredSystemFeatures is sufficient** — no custom resource labels
    beyond Nix's existing `requiredSystemFeatures` mechanism (`kvm`,
    `big-parallel`, `nixos-test`, etc.). Builders register their
    supported features in the `nodes` table.

14. **Two-step GC** — retention cleanup deletes old evaluations from
    PostgreSQL, cascading to orphan derivations/outputs. S3 sweep
    deletes objects not in `derivation_outputs` (no separate object
    tracking table — the derivation graph *is* the live set).

15. **In-tree config via `.harmonia-ci.toml`** — `attrs` (what to
    evaluate) and `retention` hints are defined in the repo, not the
    database. Developers own what gets built; admins own scheduling
    and access policy. Security-sensitive settings (priority, PR trust)
    remain server-side only.

16. **Project discovery from forge** — repositories are synced from
   forge APIs (GitHub App installations, Gitea token) into PostgreSQL.
   Admins enable projects via a searchable web UI toggle — no static
   TOML project list. Per-project settings (scheduling shares,
   PR trust) are stored in the `projects` table and editable from the
   UI.

17. **Frankenbuild prevention** — the binary cache is the single source
   of truth. Builders validate local input NAR hashes against the
   database before building and re-substitute from the cache on
   mismatch. Mismatches are logged for reproducibility tracking.

18. **IFD allowed by default** — `nix-eval-jobs` can build derivations
    mid-eval via the local nix-daemon. Projects opt out via
    `.harmonia-ci.toml` `allow-ifd = false`.

19. **Eval timeout** — server-global default (configurable), overridable
    per-project in `.harmonia-ci.toml`. Partial results are rolled back
    on timeout or crash.

20. **Eval failure rolls back** — if `nix-eval-jobs` crashes mid-stream,
    `eval_attrs` rows are deleted. Orphaned `build_jobs` are cleaned up
    by GC. No partial eval results are reported.

21. **Webhook idempotency** — `UNIQUE(project_id, commit_sha, branch)`
    on `evaluations` prevents duplicate evals from redelivered webhooks.

22. **Node draining** — `nodes.draining` flag. Draining nodes finish
    in-flight work but stop claiming new jobs. Triggered via API or CLI.

23. **Manual cancel** — users can cancel evals and builds from the UI.
    Cancelling an eval also cancels its exclusively-owned pending builds.

24. **UNLOGGED build_log_tails** — live log tails (64KB rolling buffer)
    are stored in a separate `UNLOGGED` table to avoid WAL overhead and
    TOAST churn on the `build_jobs` table. If PG crashes, tails are lost
    but full logs survive in builder memory and are uploaded to S3.

25. **Batched .drv uploads during eval** — the evaluator batches
    derivations (up to 50 or 2s) before uploading to S3. Known drvs
    (already in `derivations` table) are skipped entirely. Presigned
    URLs are requested in bulk. This avoids the O(n) sequential
    roundtrip overhead of per-attr uploads.

26. **Frankenbuild validation uses local Nix SQLite** — builders compare
    canonical hashes (from PG) against their local Nix DB, not by
    recomputing NAR hashes from store contents. O(1) per path, not
    O(size). The threat model is cross-builder non-reproducibility,
    not local filesystem corruption. Mismatches are logged as warnings;
    a dedicated tracking table is deferred.

27. **drv_json deferred** — `derivations.drv_json` is not populated
    during eval (the `nix derivation show` subprocess adds ~50ms per
    attr). Builders get .drv files from S3. The column is reserved for
    Phase 5 (PostgreSQL eval store plugin).

28. **Dependency failure propagation** — when a build fails, all
    transitively-dependent pending builds are marked `dep-failed` via
    recursive CTE. Rebuilding a failed build cascades reset to its
    `dep-failed` dependents.

29. **Failed builds not auto-retried across evals** — `UNIQUE(drv_path)`
    + `ON CONFLICT DO NOTHING` means a new eval producing the same drv
    as a failed build does not retry it. Manual "Rebuild failed" is the
    escape hatch.

30. **Max-silent timeout** — global config (default 30m). Kills builds
    with no log output, catching stuck builds much faster than the 4h
    wall-clock timeout.

31. **Polling fallback** — per-project `poll_interval` (default 0 =
    disabled). Frontend periodically checks forge for new commits as a
    safety net for missed webhooks. Coexists with webhooks via the
    UNIQUE constraint on evaluations.
