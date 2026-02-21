# Database Design

## Principle: Two Clearly Separated Layers

The database has two layers with different concerns that happen to share
`drv_path` as a join key. They must not be conflated.

## Layer 1: Nix Store Schema (PostgreSQL)

A relational mirror of what Nix's own SQLite (`/nix/var/nix/db/db.sqlite`)
tracks — but shared across all coordinators. This layer knows nothing
about CI. It is what a future `harmonia-store-pg` PostgreSQL store backend
would need in order to implement `writeDerivation` / `readDerivation` /
`queryPathInfo`.

```sql
-- Content-addressed derivations, keyed by their store path.
-- Mirrors Nix's ValidPaths table but for .drv files only.
-- Source paths and output paths remain in the local Nix store on workers.
CREATE TABLE derivations (
    drv_path          TEXT PRIMARY KEY,   -- "/nix/store/abc...-foo.drv"
    hash_part         TEXT NOT NULL,      -- first 32 chars of basename (for queryPathFromHashPart)
    name              TEXT NOT NULL,      -- "foo" (the derivation name)
    system            TEXT NOT NULL,      -- "x86_64-linux", "aarch64-darwin", …
    required_features TEXT[] NOT NULL DEFAULT '{}',  -- from requiredSystemFeatures
    drv_json          JSONB,              -- full serialised Derivation struct (for readDerivation)
                                         -- NOT populated during eval (avoid ~50ms subprocess
                                         -- call per attr). Builders get .drv from S3.
                                         -- Reserved for Phase 5 (PG eval store plugin).
    nar_hash          TEXT NOT NULL,      -- "sha256:…" (NAR hash of the .drv file)
    nar_size          BIGINT NOT NULL,
    created_at        TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX ON derivations (hash_part);
CREATE INDEX ON derivations (system);   -- fast lookup by arch for job routing

-- Mirrors Nix's DerivationOutputs table.
CREATE TABLE derivation_outputs (
    drv_path    TEXT NOT NULL REFERENCES derivations(drv_path) ON DELETE CASCADE,
    output_name TEXT NOT NULL,      -- "out", "dev", "lib", …
    output_path TEXT,               -- NULL for CA/floating derivations
    nar_hash    TEXT,               -- "sha256:…" canonical NAR hash (set after first successful build)
    nar_size    BIGINT,             -- canonical NAR size
    PRIMARY KEY (drv_path, output_name)
);

-- Mirrors Nix's Refs table, but for derivation-to-derivation edges.
-- Represents the inputDrvs graph.
CREATE TABLE derivation_inputs (
    referrer    TEXT NOT NULL REFERENCES derivations(drv_path) ON DELETE CASCADE,
    input_drv   TEXT NOT NULL,      -- may not be in this table (fetched from local store)
    outputs     TEXT[] NOT NULL,    -- ["out", "dev"]
    PRIMARY KEY (referrer, input_drv)
);

CREATE INDEX ON derivation_inputs (input_drv);
```

**What goes here from nix-eval-jobs output:**

| nix-eval-jobs field          | destination                               |
|------------------------------|-------------------------------------------|
| `drvPath`                    | `derivations.drv_path` (PK)               |
| `name`                       | `derivations.name`                        |
| `system`                     | `derivations.system`                      |
| `requiredSystemFeatures`     | `derivations.required_features`           |
| `outputs`                    | `derivation_outputs` rows                 |
| `inputDrvs`                  | `derivation_inputs` rows                  |
| via `nix derivation show`    | `derivations.drv_json` (deferred — not populated during eval) |

## Layer 2: CI Schema

Pure CI state. References `derivations.drv_path` as a foreign key but
owns none of the Nix semantics.

### Repositories and projects

```sql
-- Discovered from forge APIs. Synced periodically.
CREATE TABLE repositories (
    id             BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    forge          TEXT NOT NULL,         -- forge name from config (e.g. "github")
    owner          TEXT NOT NULL,
    name           TEXT NOT NULL,
    full_name      TEXT NOT NULL,         -- "owner/repo"
    clone_url      TEXT NOT NULL,
    is_private     BOOLEAN NOT NULL DEFAULT false,
    default_branch TEXT NOT NULL DEFAULT 'main',
    last_synced    TIMESTAMPTZ DEFAULT now(),
    UNIQUE (forge, full_name)
);

-- Enabled CI projects. One per repo (UNIQUE on repo_id).
CREATE TABLE projects (
    id             BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    repo_id        BIGINT NOT NULL REFERENCES repositories(id) UNIQUE,
    name           TEXT NOT NULL UNIQUE,  -- short name, default = repo name
    enabled        BOOLEAN NOT NULL DEFAULT true,
    -- scheduling
    scheduling_shares INT NOT NULL DEFAULT 100,  -- relative weight; higher = more builder time
    consumed_seconds  FLOAT NOT NULL DEFAULT 0,  -- build-seconds consumed (decayed hourly)
    -- PR policy
    pr_enabled     BOOLEAN NOT NULL DEFAULT true,
    pr_trust       TEXT NOT NULL DEFAULT 'collaborators',  -- collaborators | all | none
    poll_interval  INT NOT NULL DEFAULT 0,  -- seconds between polls (0 = webhook-only)
    created_at     TIMESTAMPTZ DEFAULT now()
);
-- NOTE: attrs and retention are read from in-tree .harmonia-ci.toml
-- at eval time, not stored here. See gc-and-retention.md.
```

```sql
-- One flake evaluation triggered by a webhook or manual request.
CREATE TABLE evaluations (
    id           BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    project_id   BIGINT NOT NULL REFERENCES projects(id),
    flake_url    TEXT NOT NULL,      -- "github:owner/repo/abc123"
    commit_sha   TEXT NOT NULL,      -- full commit SHA (for forge status reporting + UI display)
    branch       TEXT NOT NULL,      -- "main", "refs/pull/42/merge", …
    tree_hash    TEXT,               -- git tree hash (git rev-parse $sha^{tree})
                                     -- used for merge queue dedup
    status       TEXT NOT NULL DEFAULT 'queued'
                 CHECK (status IN ('queued', 'running', 'succeeded', 'failed', 'cancelled', 'skipped')),
    skipped_reason TEXT,             -- set when status='skipped' (e.g. 'tree_hash_hit')
    reuses_eval_id BIGINT REFERENCES evaluations(id),  -- eval whose results we reuse
    -- claiming (was separate eval_jobs table, but relationship is always 1:1)
    claimed_by   TEXT,               -- node id that claimed this eval
    claimed_at   TIMESTAMPTZ,
    started_at   TIMESTAMPTZ,        -- set by evaluator on claim (not on INSERT)
    finished_at  TIMESTAMPTZ,
    error        TEXT,               -- set on eval failure
    -- status reporting dedup (see high-availability.md)
    last_status    TEXT,             -- last forge status reported ('pending', 'success', 'failure')
    last_status_at TIMESTAMPTZ,
    UNIQUE (project_id, commit_sha, branch)  -- webhook idempotency
);

CREATE INDEX ON evaluations (project_id, branch, status);
CREATE INDEX ON evaluations (project_id, tree_hash, status);  -- fast tree dedup lookup
CREATE INDEX ON evaluations (status) WHERE status = 'queued'; -- eval claim query
CREATE INDEX ON evaluations (claimed_by) WHERE status = 'running'; -- stale eval reaping

-- One attribute from one evaluation: the mapping flake attr → drv.
-- This is NOT the build job. It records what the evaluator produced.
CREATE TABLE eval_attrs (
    id          BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    eval_id     BIGINT NOT NULL REFERENCES evaluations(id) ON DELETE CASCADE,
    attr        TEXT NOT NULL,       -- "checks.x86_64-linux.my-test"
    drv_path    TEXT REFERENCES derivations(drv_path),
    error       TEXT,                -- set when this attr failed to evaluate
    UNIQUE (eval_id, attr)
);

-- The build queue: one row per drv that needs to be built.
-- UNIQUE (drv_path) means two evals that produce the same drv share one
-- build job — no duplicate work.
CREATE TABLE build_jobs (
    id           BIGINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    drv_path     TEXT NOT NULL REFERENCES derivations(drv_path),
    project_id   BIGINT REFERENCES projects(id),  -- denormalised for shares-based scheduling
                                     -- set at INSERT from the eval that created this job;
                                     -- NULL for orphaned builds; not updated on dedup conflict
    system       TEXT NOT NULL,      -- denormalised from derivations for fast queue queries
    status       TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'building', 'uploading', 'succeeded', 'failed', 'cancelled', 'dep-failed')),
                                     --
                                     -- pending   → building   : claimed by builder, harmonia-daemon build starts
                                     -- building  → uploading  : build succeeded, NAR compress + S3 upload begins
                                     -- uploading → succeeded  : narinfo written to S3 by signer
                                     -- pending/building → failed : non-transient error or retries exhausted
                                     -- pending → cancelled    : eval superseded, build exclusively owned by cancelled eval
                                     -- pending → dep-failed   : an input build failed (transitive)
                                     --
                                     -- On builder death: heartbeat reaper resets
                                     --   building  → pending  (must rebuild)
                                     --   uploading → pending  (dead node's store is inaccessible; must rebuild)
    priority     INT NOT NULL DEFAULT 0,    -- 0 = normal, 100 = manual rebuild
    claimed_by   TEXT,               -- node id (from nodes.id)
    claimed_at   TIMESTAMPTZ,
    started_at   TIMESTAMPTZ,
    finished_at  TIMESTAMPTZ,
    exit_code    INT,
    retry_count  INT NOT NULL DEFAULT 0,  -- incremented on transient failure / stale reclaim
    log_url      TEXT,               -- S3 key: log/<drvname>.drv (set on completion)
    created_at   TIMESTAMPTZ DEFAULT now(),
    UNIQUE (drv_path)
);

CREATE INDEX ON build_jobs (status, priority DESC, created_at ASC)
    WHERE status = 'pending';          -- covers the claim query ordering
CREATE INDEX ON build_jobs (claimed_by)
    WHERE status IN ('building', 'uploading'); -- fast stale-job detection via dead node lookup
```

### Live build log tails (UNLOGGED)

```sql
-- Ephemeral log tail data, separated from build_jobs to avoid TOAST
-- churn on the claim query's table. UNLOGGED = no WAL overhead for
-- the ~500 UPDATEs/s at 1000 concurrent builds. If PG crashes, tails
-- are lost but full logs survive in the builder's memory buffer and
-- are uploaded to S3 on completion regardless.
CREATE UNLOGGED TABLE build_log_tails (
    build_job_id BIGINT PRIMARY KEY REFERENCES build_jobs(id) ON DELETE CASCADE,
    log_tail     BYTEA,              -- rolling buffer: last ~64 KB of log output
    log_seq      INT NOT NULL DEFAULT 0  -- bumped on each update (NOTIFY trigger)
);
```

### NOTIFY trigger for log streaming

```sql
CREATE OR REPLACE FUNCTION notify_build_log() RETURNS trigger AS $$
BEGIN
    IF NEW.log_seq IS DISTINCT FROM OLD.log_seq THEN
        PERFORM pg_notify('build_log', NEW.build_job_id::text);
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER build_log_notify
    AFTER UPDATE ON build_log_tails
    FOR EACH ROW EXECUTE FUNCTION notify_build_log();
```



### Node registration

```sql
CREATE TABLE nodes (
    id           TEXT PRIMARY KEY,       -- hostname, e.g. "build-x86-01"
    capabilities TEXT[] NOT NULL,        -- ["evaluator"], ["builder"], ["signer"], …
    endpoint     TEXT NOT NULL,          -- "https://build-x86-01.example.com:7777"
                                         -- used by builders to reach signer nodes
    -- evaluator-specific
    max_evals    INT NOT NULL DEFAULT 1,
    -- builder-specific
    systems      TEXT[] NOT NULL DEFAULT '{}',   -- ["x86_64-linux", "aarch64-linux"]
    features     TEXT[] NOT NULL DEFAULT '{}',   -- ["kvm", "big-parallel", "nixos-test"]
    max_jobs     INT NOT NULL DEFAULT 1,
    speed_factor FLOAT NOT NULL DEFAULT 1.0,
    -- common
    last_seen    TIMESTAMPTZ DEFAULT now(),
    draining     BOOLEAN NOT NULL DEFAULT false  -- true = finish in-flight, claim nothing new
);
```

## Entity Relationship

```
repositories ──► projects ──► evaluations
                                    │
nodes (fleet)                       └────────► eval_attrs ──► derivations ◄── build_jobs
  │ claimed_by                                                     │           │
  │                                                     derivation_inputs   build_log_tails
  │                                                     derivation_outputs  (UNLOGGED)
  │

```

## What Is NOT Stored

The following fields from nix-eval-jobs are **ephemeral dispatch signals**,
not stored in either layer:

| field              | use                                          |
|--------------------|----------------------------------------------|
| `cacheStatus`      | decide whether to INSERT into `build_jobs`   |
| `neededBuilds`     | same                                         |
| `neededSubstitutes`| same                                         |

If `cacheStatus = local` or `cacheStatus = cached`, no `build_jobs` row
is inserted. These values are recomputed on each eval run.
