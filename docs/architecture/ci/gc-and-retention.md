# Garbage Collection and Retention

Since harmonia-ci replaces niks3, it inherits responsibility for binary
cache garbage collection. The S3 bucket is not just CI artifacts — it is
the production binary cache that users substitute from.

## GC Model: Two-Step Cleanup

GC is two independent steps:

1. **Retention cleanup** — delete old evaluations from PostgreSQL.
   Cascading deletes remove `eval_attrs`, which orphans `derivations`
   and `derivation_outputs` rows no longer referenced by any evaluation.

2. **S3 sweep** — delete S3 objects not represented in
   `derivation_outputs` (for NARs/narinfo) or `build_jobs` (for logs).

No separate object tracking table is needed. `derivation_outputs`
already records every store path and its NAR hash — from these two
columns we can derive every S3 key deterministically:

```
derivation_outputs.output_path  →  <hash>.narinfo
derivation_outputs.nar_hash     →  nar/<nar_hash>.nar.zst
build_jobs.log_url              →  log/<drvname>.drv
```

### Step 1: Retention cleanup (PostgreSQL)

```sql
-- Delete old evaluations (cascades to eval_attrs)
DELETE FROM evaluations
WHERE started_at < now() - $1::interval  -- per-project retention
  AND project = $2;

-- Orphan cleanup: remove build_jobs no longer referenced by any eval.
-- Must run before derivations cleanup due to FK constraint.
-- Guard: never delete jobs that are actively building or uploading —
-- the builder still holds the claimed row and its local store has outputs.
DELETE FROM build_jobs bj
WHERE NOT EXISTS (
    SELECT 1 FROM eval_attrs ea WHERE ea.drv_path = bj.drv_path
)
  AND bj.status NOT IN ('building', 'uploading');

-- Orphan cleanup: remove derivations no longer referenced by any eval
-- (cascades to derivation_outputs, derivation_inputs)
DELETE FROM derivations d
WHERE NOT EXISTS (
    SELECT 1 FROM eval_attrs ea WHERE ea.drv_path = d.drv_path
);
```

### Step 2: S3 sweep

Compute the live key set from remaining `derivation_outputs` rows,
then delete everything else from S3:

```sql
-- All live S3 keys (computed from derivation_outputs + build_jobs)
SELECT substring(output_path from '/nix/store/(.{32})') || '.narinfo' AS key
FROM derivation_outputs WHERE output_path IS NOT NULL
UNION
SELECT 'nar/' || substring(nar_hash from 'sha256:(.*)') || '.nar.zst'
FROM derivation_outputs WHERE nar_hash IS NOT NULL
UNION
SELECT log_url FROM build_jobs WHERE log_url IS NOT NULL;
```

```
1. Load live_keys set into memory (or temp table)
2. S3 ListObjectsV2 (paginated)
3. For each S3 object not in live_keys:
     - if LastModified < now() - grace_period (24h): delete
       (grace period protects in-flight uploads)
     - else: skip
```

### Why no object tracking table?

niks3 needs an `objects` table because it has no derivation graph — it
only knows about S3 keys and their reference chains. Harmonia-ci already
has the full graph in `derivation_outputs`, making a separate table
redundant. This eliminates:

- A `cache_objects` table and its maintenance
- INSERT on every upload, DELETE on every GC sweep
- Resurrection race handling (`first_deleted_at` / `deleted_at` pair)
- Recursive CTE to walk object references

The tradeoff: GC must list the S3 bucket to find objects to delete.
For caches with millions of objects the list operation takes minutes,
but it runs infrequently (daily) and is paginated.

## Retention Policy

Retention is configured globally with per-project overrides:

```toml
[retention]
default = "90d"        # keep evals/builds/NARs for 90 days
log_retention = "30d"  # build logs deleted sooner (large, less useful over time)
```

Per-project retention overrides come from `.harmonia-ci.toml` in-tree
(not the server config — projects are not listed in TOML, see
[forge-integration.md](forge-integration.md#project-discovery-and-onboarding)):

```toml
# .harmonia-ci.toml (in the repo)
retention = "180d"   # this project's evals kept longer than server default
```

### What gets cleaned up

| Data | Retention | Mechanism |
|---|---|---|
| `evaluations`, `eval_attrs` rows | Per-project retention window | `DELETE FROM evaluations WHERE started_at < now() - $1` (cascades) |
| `build_jobs` rows | Orphan cleanup | Delete rows whose `drv_path` is no longer referenced by any `eval_attrs` |
| `derivations`, `derivation_outputs`, `derivation_inputs` | Orphan cleanup | Delete rows whose `drv_path` is no longer referenced by any `eval_attrs` or `build_jobs` |

| `*.narinfo`, `nar/*.nar.zst` in S3 | GC mark-sweep | Objects not in live set, past grace period |
| `log/*.drv` in S3 | `log_retention` (shorter) | Same GC mechanism, or S3 lifecycle rule on `log/` prefix |

### Retention and GC interaction

The retention window determines which evaluations are GC roots. When an
evaluation ages past the retention window, it is deleted from PostgreSQL.
Its derivation outputs may become unreachable — the next GC mark phase
will not include their S3 keys in the live set, and the sweep phase
deletes them from S3 (after the grace period).

Actual S3 object lifetime in the worst case:
`retention_window + gc_interval + grace_period`.

**GC vs. concurrent eval race**: GC orphan cleanup could delete a
derivation that a concurrent eval is about to reference. This is safe
because: (1) the eval's `INSERT INTO derivations ON CONFLICT DO NOTHING`
re-inserts the row if GC deleted it, (2) the .drv S3 upload is guarded
by the signer's HEAD check — if the object survived the S3 grace period,
it's re-uploaded; if not, the evaluator re-uploads via presigned URL.
The 24h S3 grace period makes the object-level race practically
impossible under normal GC intervals (daily).

## In-Tree Configuration

Projects can override evaluation and retention settings via a
`.harmonia-ci.toml` file in the repository root:

```toml
# .harmonia-ci.toml

# Which flake outputs to evaluate (default: ["checks"])
attrs = ["checks", "packages"]

# Retention override (optional, falls back to server default)
retention = "180d"

# Allow import-from-derivation during eval (default: true)
# Set to false to ban IFD — attrs requiring IFD will fail to evaluate
allow-ifd = true

# Eval timeout override (default: server global, typically 1h)
# eval-timeout = "2h"
```

If `.harmonia-ci.toml` is missing, the evaluator uses defaults:
`attrs = ["checks"]`, `allow-ifd = true`, server-default retention.

The evaluator reads this file from the flake source before running
`nix-eval-jobs`. This is the **only** place `attrs` and IFD policy
are configured — they are not in the database or web UI. Server-side
project config takes precedence for security-sensitive settings
(priority, PR trust); in-tree config is limited to eval scope,
IFD policy, and retention hints.
