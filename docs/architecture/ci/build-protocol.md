# Work-Stealing Build Protocol

## Why Not SSH

The existing Nix remote builder protocol (`build-remote` + `nix-store
--serve`) has several structural problems:

1. **Push scheduling** — the coordinator pushes jobs to workers via a
   build hook. A free worker cannot pull the next job; it waits to be
   offered one. Idle capacity is wasted.

2. **Coordinator-local slot tracking** — available slots are tracked via
   lock files in a `currentLoad/` directory on the coordinator's
   filesystem. This is not shared state: multiple coordinators cannot see
   each other's slot usage.

3. **Serialised uploads per worker** — a global per-machine upload lock
   serialises all input uploads to a given worker. Two jobs going to the
   same worker must queue their uploads sequentially even if inputs are
   disjoint.

4. **Uncompressed NAR streams** — NAR data is sent raw over SSH stdio.
   SSH's optional `-C` compression is gzip and not NAR-aware. A 1.3 GB
   stdenv closure is retransferred in full on a cold worker.

5. **Binary cache is a single bottleneck** — all output uploads go through
   one harmonia-cache instance. Every worker streams its built NARs back
   through the coordinator's machine, even when an S3-backed store is the
   final destination.

6. **`speedFactor` is static config** — the scheduler uses a fixed value
   from `nix.conf`. No feedback loop: a thermally throttled machine still
   receives jobs at its nominal rate.

## Design Goals

- **Pull-based / work-stealing**: builder nodes pull jobs when ready,
  using PostgreSQL `FOR UPDATE SKIP LOCKED`. No central scheduler.
- **All state in PostgreSQL**: capacity, heartbeats, job status — no
  local files on any node. Any node can restart and resume cleanly.
- **Output uploads bypass the signer entirely**: builders upload directly
  to S3 via presigned PUT URLs. The signer signs the URL but never
  touches NAR bytes.
- **Inputs substituted from S3/cache**: builders pull build inputs from
  the shared binary cache, not from another node.
- **zstd compression**: NAR data is stored and transferred compressed.

## Scheduling and Priorities

Build scheduling solves two distinct problems:

1. **Readiness** — can this job build right now? (Are its inputs available?)
2. **Priority** — among ready jobs, which one should a builder pick first?

### DAG-Aware Readiness

A job is claimable only when all its input derivations are satisfied:
either already in the cache (no `build_jobs` row) or built successfully
(`status = 'succeeded'`). This is enforced in the claim query via a
`NOT EXISTS` subquery against `derivation_inputs`.

This gives topological build order for free — leaf derivations become
ready first, then their dependents, naturally producing a wavefront that
advances up the DAG. It also avoids wasting a builder slot on a job that
would just block waiting for inputs.

When a build succeeds, its dependents may become newly ready. A `NOTIFY`
wakes builders to re-run the claim query:

```sql
-- after marking job succeeded:
NOTIFY build_jobs, '{"system":"x86_64-linux"}';
```

### Scheduling Fairness (Shares)

Inter-project fairness is handled by **scheduling shares**. Each project
has a `scheduling_shares` value (default 100). The claim query sorts by
`consumed_seconds / shares` ascending — the project that has consumed
the least builder time relative to its share goes first.

```
project       shares    consumed_seconds    ratio
clan-core     200       4000                20.0
dotfiles      100       1500                15.0  ← picked first
nixpkgs       300       9000                30.0
```

This prevents a large project from monopolizing builders indefinitely.
A project with 2× the shares gets 2× the builder time, not absolute
priority over everything else.

**Consumed seconds** are tracked on the `projects` table and incremented
when a build completes:

```sql
UPDATE projects SET consumed_seconds = consumed_seconds + $duration
WHERE id = $project_id;
```

To prevent history from accumulating forever, consumed_seconds are
decayed hourly (exponential decay, factor 0.95):

```sql
UPDATE projects SET consumed_seconds = consumed_seconds * 0.95
WHERE consumed_seconds > 0;
```

This creates an effective ~13-hour half-life: recent builds weigh
heavily, but a project that was busy 24 hours ago has mostly "forgotten"
that usage. No sliding window bookkeeping needed.

### Priority: Manual Rebuild Only

Priority is a single integer on `build_jobs`, default 0. The only use
is manual rebuild, which sets `priority = 100` to escape fairness:

```
priority = 0     normal build (from eval)
priority = 100   manual rebuild (user clicked "Rebuild")
```

Builds with `priority > 0` go to the front of the queue regardless of
shares. `created_at` is the tiebreaker within the same priority/fairness
rank — older jobs first.

Branch-based priority boosts (main vs PR) were considered and rejected.
Within a project, FIFO ordering is sufficient — all builds from the
same eval are equally important.

### Full claim query sort order

```
1. priority DESC                           -- manual rebuilds first
2. consumed_seconds / shares ASC           -- inter-project fairness
3. created_at ASC                          -- FIFO
```

LPT (Longest Processing Time first) was evaluated via simulation
(see `sim/scheduling_sim.py`) and found to improve makespan by only
~1-2% with typical builder counts (4-32). The complexity is not
justified.

Shares are configured per-project in the database via the web UI
(see [forge-integration.md](forge-integration.md#enabling-projects)).

### Deduplication

`UNIQUE(drv_path)` means two evaluations producing the same derivation
share one `build_jobs` row. The second evaluation's INSERT is a no-op:

```sql
INSERT INTO build_jobs (drv_path, project_id, system, priority)
VALUES ($1, $2, $3, 0)
ON CONFLICT (drv_path) DO NOTHING;
```

The `project_id` is set by the first eval that creates the row.
Priority only changes via manual rebuild (`priority = 100`).

## Work-Stealing via PostgreSQL

Builder nodes poll for work using `FOR UPDATE SKIP LOCKED`, initiated by
the **builder**, not by any central scheduler:

```sql
WITH ready AS (
    SELECT bj.id, bj.drv_path, d.system, d.required_features
    FROM build_jobs bj
    JOIN derivations d USING (drv_path)
    LEFT JOIN projects p ON p.id = bj.project_id
    WHERE bj.status = 'pending'
      AND d.system            = ANY($1::text[])  -- worker's supported systems
      AND d.required_features <@ $2::text[]      -- worker's supported features
      AND NOT EXISTS (
          -- all input drvs that need building must be done
          SELECT 1 FROM derivation_inputs di
          JOIN build_jobs dep ON dep.drv_path = di.input_drv
          WHERE di.referrer = bj.drv_path
            AND dep.status NOT IN ('succeeded')
      )
    ORDER BY
             bj.priority DESC,                    -- manual rebuilds first
             COALESCE(p.consumed_seconds / GREATEST(p.scheduling_shares, 1), 0) ASC,  -- fairness
             bj.created_at ASC                    -- FIFO
    LIMIT 1
    FOR UPDATE OF bj SKIP LOCKED
)
UPDATE build_jobs SET
    status     = 'building',
    claimed_by = $3,
    claimed_at = now()
FROM ready
WHERE build_jobs.id = ready.id
RETURNING ready.*;
```

No central scheduler: any idle builder with matching capabilities picks
up the highest-priority ready job. No lock files. Works correctly with
any number of harmonia-ci nodes running simultaneously.

**Performance note**: The `NOT EXISTS` subquery checks readiness per
candidate row in priority order. If many high-priority jobs are blocked
on the same long-running dependency, PG scans past all of them before
finding a claimable job. In practice this is bounded by the wavefront
size (~50–200 blocked jobs) and the partial index keeps the scan fast.
If this becomes a bottleneck at scale, a materialized `ready` boolean
column (maintained by a trigger on `build_jobs` status changes) would
eliminate the subquery, at the cost of trigger complexity.

## Input Substitution: Pull from S3

Build inputs come from the shared S3 binary cache. Builder nodes
configure:

```toml
# /etc/nix/nix.conf on each builder node
substituters = https://cache.example.com   # harmonia-cache in front of S3
trusted-public-keys = cache.example.com-1:...
```

When a builder claims a job, Nix's normal substitution mechanism fetches
missing input paths from S3 via harmonia-cache. Hot paths (stdenv,
nixpkgs bootstrap) are already in S3 from previous builds and are fetched
at full S3/CDN bandwidth — no inter-node transfer.

For inputs not yet in S3, builders fall back to fetching from upstream
(cache.nixos.org) as usual.

## Frankenbuild Prevention

Non-reproducible builds create a subtle correctness hazard: if builder A
previously built store path `/nix/store/abc-foo` and got NAR hash H1,
but the binary cache holds the same store path with NAR hash H2, then
any build on builder A that depends on `abc-foo` uses the wrong inputs.
The result is a **Frankenbuild** — a package built against inputs that
differ from the canonical cache, producing outputs that are neither
reproducible nor consistent with any other builder.

### The binary cache is the single source of truth

The S3 binary cache defines the canonical NAR hash for every store path.
The database records these canonical hashes in
[`derivation_outputs.nar_hash`](database.md#layer-1-nix-store-schema-postgresql).

### Builder input validation

Before building, the builder must verify that all locally-present input
paths match the canonical hashes recorded in the database. The builder
reads NAR hashes from its **local Nix SQLite database**
(`/nix/var/nix/db/db.sqlite` — the `hash` column in `ValidPaths`),
NOT by recomputing hashes from store path contents. This makes the
check O(1) per path (a single SQLite lookup) rather than O(size) for
NAR serialisation + SHA256.

The threat model is: two builders independently built the same
derivation and got different outputs (non-reproducibility). The local
SQLite hash is trustworthy because the local nix-daemon computed it at
build time. We are not defending against local filesystem corruption.

```
builder claims job
    │
    ▼
for each input store path required by the derivation:
    │
    ├─ path missing locally?
    │   → substitute from cache (normal Nix substitution)
    │
    ├─ path present locally, no canonical hash in DB yet?
    │   → first build of this path, accept local copy
    │   → (its hash becomes canonical when uploaded to cache)
    │
    └─ path present locally, canonical hash exists in DB?
        │
        ├─ local SQLite hash matches DB → ok, use local copy
        │
        └─ local SQLite hash differs → MISMATCH
            → log warning (builder stderr)
            → delete local path (nix-store --delete)
            → re-substitute from cache
            → if substitution fails: mark job as failed
```

This validation runs in the builder binary before invoking the
harmonia-daemon for the actual build. It ensures every input is either
canonical (matches the cache) or is the first build (will become
canonical upon upload).

### Recording canonical hashes

When a build succeeds and outputs are uploaded to S3:

```sql
UPDATE derivation_outputs
SET nar_hash = $1, nar_size = $2
WHERE drv_path = $3 AND output_name = $4
  AND nar_hash IS NULL;  -- first writer wins: the cache's version is canonical
```

The `AND nar_hash IS NULL` guard ensures that once a canonical hash is
set, it is never overwritten. If two builders race to build the same
derivation, the first to upload wins and defines the canonical hash.
The second builder's output is silently discarded (the cache already
has the path).

### Why not just always substitute?

Substituting every input from the cache on every build would be correct
but wasteful — most locally-present paths are fine (especially on
dedicated builders that only run CI builds). The validation step is a
lightweight check (compare hashes from the DB, no network I/O unless
mismatch) that avoids unnecessary downloads while catching the rare
divergence.

### Non-reproducibility detection

Mismatches are logged as warnings to builder stderr. A dedicated
reproducibility tracking table is deferred to a later phase.

## Unsupported Build Detection

A build is "unsupported" if no live node can run it — either no node
supports its `system` or no node has all its `required_features`. These
builds sit in `pending` forever because the claim query never matches.

A periodic reaper (runs every 5 minutes on any frontend node) detects
and fails them:

```sql
UPDATE build_jobs SET status = 'failed', exit_code = NULL,
    finished_at = now()
WHERE status = 'pending'
  AND created_at < now() - interval '30 minutes'  -- grace period for nodes to come online
  AND NOT EXISTS (
      SELECT 1 FROM nodes n
      WHERE n.last_seen > now() - interval '2 minutes'
        AND n.draining = false
        AND (SELECT system FROM derivations WHERE drv_path = build_jobs.drv_path) = ANY(n.systems)
        AND (SELECT required_features FROM derivations WHERE drv_path = build_jobs.drv_path) <@ n.features
  );
```

The 30-minute grace period allows time for nodes to register after a
cluster restart. Unsupported builds trigger dependency failure
propagation (see below), so their dependents are also marked `dep-failed`.

## Dependency Failure Propagation

When a build fails permanently, all transitively-dependent pending
builds are marked `dep-failed` — they can never succeed because an
input is broken. This avoids leaving dead builds in "pending" state,
giving users immediate feedback.

```sql
-- After marking build $1 as 'failed':
WITH RECURSIVE to_fail AS (
    -- seed: the build that just failed
    SELECT bj.drv_path
    FROM build_jobs bj
    WHERE bj.id = $1 AND bj.status = 'failed'
  UNION
    -- walk UP the DAG: find pending builds that depend on any failed drv
    SELECT di.referrer
    FROM derivation_inputs di
    JOIN to_fail tf ON tf.drv_path = di.input_drv
    JOIN build_jobs bj ON bj.drv_path = di.referrer
    WHERE bj.status = 'pending'
)
UPDATE build_jobs SET status = 'dep-failed'
WHERE drv_path IN (SELECT drv_path FROM to_fail)
  AND status = 'pending'
RETURNING drv_path;

-- NOTIFY for each dep-failed build so frontend updates forge status
-- (batched, same as regular build completion)
NOTIFY build_jobs, '{"drv_path": "...", "status": "dep-failed"}';
```

The UI shows `dep-failed` builds with a link to the root-cause failure.

**Rebuild interaction**: When a failed build is reset to `pending`
(via "Rebuild failed"), its `dep-failed` dependents are also reset to
`pending`. The claim query's NOT EXISTS check naturally prevents them
from being claimed until all deps are satisfied — no premature execution.

```sql
-- When resetting a failed build to pending (rebuild):
WITH RECURSIVE to_reset AS (
    SELECT drv_path FROM build_jobs WHERE id = $1
  UNION
    SELECT di.referrer FROM derivation_inputs di
    JOIN to_reset tr ON tr.drv_path = di.input_drv
    JOIN build_jobs bj ON bj.drv_path = di.referrer
    WHERE bj.status = 'dep-failed'
)
UPDATE build_jobs SET status = 'pending'
WHERE drv_path IN (SELECT drv_path FROM to_reset)
  AND status = 'dep-failed';
```

## Failure Handling and Retries

Failures are classified as transient or permanent:

- **Transient**: OOM, disk full, substitution network timeout, builder
  crash (detected by heartbeat reaper). The job is reset to `pending`
  and `retry_count` is incremented.
- **Permanent**: harmonia-daemon exits with non-zero status (build error).
  The job moves to `failed` and stays there until a user triggers a
  manual rebuild. Because `build_jobs` has `UNIQUE(drv_path)`, a failed
  build is **not automatically retried** when a new evaluation produces
  the same derivation — the evaluator's `INSERT ... ON CONFLICT DO
  NOTHING` silently skips it. The new eval's `eval_attrs` still
  references the drv_path, so the aggregate status correctly reflects
  the failure. The escape hatch is "Rebuild failed" from the UI, which
  resets the job to `pending` (and cascades to `dep-failed` dependents).

Auto-retry is capped at 3 attempts:

```sql
-- Heartbeat reaper (transient failure: builder died)
-- Covers both 'building' and 'uploading' — a dead node's local store
-- is inaccessible, so uploads in progress must also be rebuilt.
-- If NARs were already uploaded to S3, the signer's HEAD check skips
-- them on the next attempt.
UPDATE build_jobs SET
    status      = 'pending',
    claimed_by  = NULL,
    claimed_at  = NULL,
    retry_count = retry_count + 1
WHERE status    IN ('building', 'uploading')
  AND claimed_by IN (
      SELECT id FROM nodes WHERE last_seen < now() - interval '2 minutes'
  )
  AND retry_count < 3;

-- Jobs that exhausted retries → permanent failure
UPDATE build_jobs SET status = 'failed'
WHERE status    IN ('building', 'uploading')
  AND claimed_by IN (
      SELECT id FROM nodes WHERE last_seen < now() - interval '2 minutes'
  )
  AND retry_count >= 3;
```

## Build Timeouts

Two timeouts apply to every build, enforced locally by the builder:

1. **Wall-clock timeout** (default: 4 hours) — kills the build if it
   exceeds total elapsed time. Catches infinite loops.

2. **Max-silent timeout** (default: 30 minutes) — kills the build if
   no log output is produced for this duration. Catches stuck builds
   (deadlocks, hung network, waiting on a resource) much faster than
   the wall-clock timeout.

Additionally, two size limits prevent resource abuse:

3. **Max output size** (default: 4 GB) — if the total NAR size of all
   build outputs exceeds this limit, the build is marked `failed`. Prevents
   a single derivation from filling S3 or local disk.

4. **Max log size** (default: 64 MB) — if the in-memory log buffer exceeds
   this limit, the builder stops appending and truncates with a marker
   (`[... log truncated at 64 MB]`). The build continues — this is not a
   failure, just a log cap. Prevents OOM on the builder from derivations
   that produce unbounded stdout (e.g. verbose test suites).

All are global defaults configured in server TOML:

```toml
[build]
timeout = "4h"
max_silent = "30m"
max_output_size = "4GB"
max_log_size = "64MB"
```

The builder resets the max-silent timer on each log line received from
harmonia-daemon. Whichever timeout fires first kills the build, which
is marked `failed` with an appropriate error message ("build timed out"
/ "no output for 30 minutes").

Timeouts are enforced locally by the builder, not by the reaper — if
the builder itself dies, the node heartbeat reaper handles it (see
[high-availability.md](high-availability.md)).

## Garbage Collection on Builders

Builders register active build inputs as GC roots for the duration of
the build. This allows concurrent garbage collection (via external cron
or nix-collect-garbage) without risk of inputs being deleted mid-build.

The builder creates temporary GC roots before invoking the harmonia-daemon:

```
/nix/var/nix/gcroots/harmonia-ci/<job-id> → /nix/store/<input-path>
```

On build completion (success or failure), the GC root symlinks are
removed. If the builder crashes, stale GC roots are harmless — they
prevent GC of some paths until the next builder restart cleans them up.

## Multi-Output Uploads

A derivation may produce multiple outputs (`out`, `dev`, `lib`, etc.).
The builder requests presigned URLs for all outputs in a single
`/api/upload-slots` call and uploads them to S3.

If any output upload fails, the builder retries the entire upload
sequence — re-requesting presigned URLs for all outputs. The signer
checks `HEAD` on each NAR key before issuing a presigned URL and skips
outputs already present in S3, so successfully-uploaded outputs are not
re-transferred.

## Capacity and Back-Pressure

Workers self-regulate: they stop claiming rows when
`current_jobs >= max_jobs`. No node tracks per-worker slot
counts. `speed_factor` is updated by the worker based on a rolling
average of recent build durations, giving the scheduler a natural bias
toward faster workers without any central coordination.

## Build Log Streaming

Build logs are streamed in real-time during builds and archived to S3 as
a single object on completion. The design must scale to thousands of
concurrent builds without adding infrastructure (no Redis, no log
aggregator).

### Architecture: PostgreSQL live tail + single S3 archive

Since the builder binary drives the harmonia-daemon directly, it receives log
output on the daemon connection as the build progresses. Two phases:

1. **During build**: builder accumulates the full log in memory and
   pushes a rolling `log_tail` (last ~64 KB) to the `build_log_tails`
   table every ~2s for live tailing in the web UI. This table is
   `UNLOGGED` to avoid WAL overhead — tails are ephemeral and survive
   only in PG memory. Separating log tails from `build_jobs` avoids
   TOAST churn on the claim query's table.
2. **On completion**: builder zstd-compresses the full log and uploads
   it as a single object to S3 at
   `s3://bucket/log/<drvname>.drv` (zstd-compressed content), matching
   the path layout that `nix log` and harmonia-cache expect. One PUT
   per build, no intermediate chunks.

### Data flow

```
builder                         PostgreSQL                 S3
  │                                  │
  │  (log lines arrive from          │
  │   harmonia-daemon every few ms)       │
  │                                  │
  │  ── every ~2s ─────────────────►│
  │  UPSERT build_log_tails          │
  │    SET log_tail = <last 64KB>,   │
  │        log_seq  = log_seq + 1    │
  │                          NOTIFY  │
  │                        build_log │
  │                                  │
  │  (build completes)               │
  │                                  │                      │
  │  PUT log/<drvname>.drv       ──────────────────────────►│
  │  (single object, full log)       │                      │
  │                                  │
  │  DELETE FROM build_log_tails     │
  │    WHERE build_job_id = $1       │
  │  UPDATE build_jobs               │
  │    SET log_url = 'log/…'         │
  └──────────────────────────────────┘

frontend (SSE)                   PostgreSQL                 S3
  │                                  │                      │
  │  LISTEN build_log                │                      │
  │◄─────────────────────────────────┤                      │
  │                                  │                      │
  │  SELECT log_tail                 │                      │
  │  FROM build_log_tails            │                      │
  │  WHERE build_job_id = 42         │                      │
  │◄─────────────────────────────────┤                      │
  │                                  │                      │
  │  (build finished, client wants full log)                │
  │  GET /log/<drv_hash>             │                      │
  │  ← harmonia-cache proxies ───────┼─────────────────────►│
  │◄─────────────────────────────────┼──────────────────────┤
```

### Scale analysis

At 1000 concurrent builds:

| Operation | Rate | Impact |
|---|---|---|
| `UPSERT build_log_tails` | 1000 builds × 1/2s = **500/s** | Low — UNLOGGED table, no WAL, no TOAST churn on build_jobs |
| `NOTIFY build_log` | Coalesced with UPDATE, **500/s** | Fine — lightweight |
| S3 PUT (final log) | **1 per build** | Negligible |

For comparison, the rejected alternatives:

| Approach | Writes/sec at 1000 builds | Problem |
|---|---|---|
| PG UNLOGGED INSERT per chunk | 2000/s INSERTs + DELETEs | Table bloat, vacuum pressure |
| S3 PUT per chunk (500ms) | 2000/s PUTs | Expensive, high latency |
| S3 PUT per chunk (30s) | 33/s PUTs | Better, but still unnecessary complexity |
| Redis | 2000/s SET | Extra dependency |

### Builder implementation

The builder maintains two buffers for the current build's log output:

1. **Log lines arrive** from the harmonia-daemon connection
2. **Append to full log buffer** (grows for the duration of the build)
3. **Every ~2s**: `INSERT INTO build_log_tails (build_job_id, log_tail, log_seq) VALUES ($1, <last 64KB>, 1) ON CONFLICT (build_job_id) DO UPDATE SET log_tail = EXCLUDED.log_tail, log_seq = build_log_tails.log_seq + 1`
4. **On completion** (success or failure):
   - zstd-compress the full log buffer
   - PUT zstd-compressed log to `s3://bucket/log/<drvname>.drv` via presigned URL from signer
   - `DELETE FROM build_log_tails WHERE build_job_id = $1`
   - `UPDATE build_jobs SET log_url = 'log/<drvname>.drv'`

The S3 key `log/<drvname>.drv` (e.g. `log/k3b2gg5n0p2q8r9t1v4w6x7y-hello-2.12.drv`)
matches the layout that harmonia-cache's `/log/<drv>` endpoint expects,
so `nix log` works against the binary cache transparently.
harmonia-cache decompresses zstd on the fly for clients that don't
accept it (same as it already does for bz2).

### Frontend: SSE endpoint

The frontend exposes `GET /api/builds/{id}/log` as a Server-Sent Events
stream:

- **Build in progress** (`status = 'building'`):
  - `LISTEN build_log` on PostgreSQL
  - On each notification: `SELECT log_tail, log_seq FROM build_log_tails WHERE build_job_id = $1`
  - Stream `log_tail` content as SSE `data:` frame with `id: <log_seq>`
  - Client reconnects with `Last-Event-ID: <log_seq>` to resume

- **Build finished**: redirect to `/log/<drvname>.drv` (served by
  harmonia-cache from S3, decompressed on the fly).

The live tail has **~2s latency** — imperceptible for log watching.
Full logs are served from S3 via harmonia-cache with no PostgreSQL load.

## Multi-System / Multi-Architecture

Workers register with their supported `systems` array. Job routing is
purely by matching `build_jobs.system` (denormalised from
`derivations.system`) against the worker's `systems` in the claim query.
No central routing logic required.

For cross-architecture builds on a single machine (binfmt_misc, Rosetta),
a worker registers with multiple systems. The worker decides what it can
run — no central node has per-architecture logic.

## Full Build Flow

```
PostgreSQL build_jobs
[pending: job-A x86_64, job-B aarch64, job-C x86_64]
        │
        │  FOR UPDATE SKIP LOCKED (worker initiates)
        ├──────────────────────────────────────────────────────┐
        │                                                      │
        ▼                                                      ▼
  worker-1 (x86_64)                                    worker-2 (x86_64)
  claims job-A                                         claims job-C
        │                                                      │
        │ build via harmonia-daemon (inputs from S3 via harmonia-cache) │ build via harmonia-daemon
        │   ├─ UPSERT build_log_tails every ~2s                  │   ├─ UPSERT build_log_tails
        │   └─ accumulate full log in memory                   │   └─ accumulate full log
        │                                                      │
        │ on completion:                                       │ on completion:
        │   compute NAR hash + compress                        │   compute NAR hash + compress
        │   PUT log/<drvname>.drv to S3, delete log tail        │   PUT log to S3, delete log tail
        │                                                      │
        │ POST /api/upload-slots  (to any signer node)                       │ POST /api/upload-slots  (to any signer node)
        │   ← presigned S3 PUT URL + signed narinfo           │   ← presigned S3 PUT URL
        │                                                      │
        │ PUT s3://bucket/nar/<hash>.nar.zst  ◄───────────────┘
        │   (direct, no signer in data path)
        │
        │ POST /api/build-complete  (to any signer node)
        │   signer writes narinfo to S3
        │   UPDATE build_jobs SET status='succeeded'
        │   reports commit status to forge
        ▼
  S3 bucket
  <hash>.narinfo              ← signed, served by harmonia-cache
  nar/<hash>.nar.zst          ← served by harmonia-cache or directly
  log/<drvname>.drv           ← build logs (zstd), served via /log/<drv>
  <hash>.ls                   ← directory listings
  realisations/<hash>!<out>.doi ← CA derivation realisations
```

## What Server-Side Roles Do and Do Not Do

| Role does | Role does NOT do |
|---|---|
| **Signer**: issues presigned S3 PUT URLs | Transfer NAR bytes |
| **Signer**: signs narinfo (holds Ed25519 key) | Receive build output |
| **Signer**: writes narinfo to S3 after confirmation | Buffer or proxy NAR data |
| **Frontend**: updates `build_jobs` status in PostgreSQL | Track per-worker slot locks |
| **Frontend**: reports to forge (commit status API) | Manage SSH connections |
| **Signer**: verifies metadata reported by worker | Re-verify S3 upload integrity |
