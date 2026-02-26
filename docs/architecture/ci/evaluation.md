# Evaluation Flow

Evaluation is distributed the same way builds are: any node with the
`evaluator` capability can run `nix-eval-jobs`. A webhook inserts an
`evaluations` row with `status='queued'`; any available evaluator node
claims it via `FOR UPDATE SKIP LOCKED`, runs `nix-eval-jobs`, streams
the JSON output directly into PostgreSQL, then marks the eval done.

## Superseding: New Push Cancels Old Eval

When a new push arrives for the same project+branch while an older
evaluation is still running or queued, the older evaluation is cancelled:

```sql
-- Cancel older evals for same project+branch
UPDATE evaluations SET status = 'cancelled', finished_at = now()
WHERE project_id = $1 AND branch = $2
  AND status IN ('queued', 'running')
  AND id < $3;  -- $3 = new eval id

-- Cancel pending build_jobs that were ONLY needed by the cancelled evals
-- (builds already in 'building' or 'uploading' finish — their outputs
-- are useful in the cache regardless)
UPDATE build_jobs SET status = 'cancelled'
WHERE status = 'pending'
  AND drv_path IN (
      -- builds referenced by the cancelled evals
      SELECT ea.drv_path FROM eval_attrs ea
      WHERE ea.eval_id IN (
          SELECT id FROM evaluations
          WHERE project_id = $1 AND branch = $2 AND status = 'cancelled'
      )
      AND ea.drv_path IS NOT NULL
  )
  AND drv_path NOT IN (
      -- but NOT referenced by any still-active eval (any project)
      SELECT ea.drv_path FROM eval_attrs ea
      JOIN evaluations e ON e.id = ea.eval_id
      WHERE e.status NOT IN ('cancelled', 'failed')
        AND ea.drv_path IS NOT NULL
  );
```

The `NOT IN` subquery ensures that pending builds shared with a
still-active evaluation are kept. Only builds exclusively owned by
cancelled evals are dropped. Builds already running finish — their
outputs enter the cache and may be reused by the new evaluation via
`UNIQUE(drv_path)` deduplication.

If the evaluator is mid-`nix-eval-jobs` when its eval is cancelled, it
checks the eval status periodically (every ~100 attributes) and aborts
early, killing the `nix-eval-jobs` subprocess.

## Tree Hash Deduplication

Before starting an evaluation, the frontend checks whether any previous
evaluation for the same project already succeeded with the same git tree
hash. If so, the eval is skipped entirely — results are reused.

```
push arrives (sha = abc123)
    │
    ├─ resolve tree hash via forge API (GitHub: GET /git/commits/$sha → tree.sha)
    │  (if API call fails: set tree_hash = NULL, skip dedup, proceed normally)
    │
    ├─ Dedup check + insert are done atomically in a single transaction
    │  to prevent TOCTOU races with concurrent evals or GC:
    │
    │  BEGIN ISOLATION LEVEL SERIALIZABLE;
    │    SELECT id INTO reuse_id FROM evaluations
    │    WHERE project_id = $1
    │      AND tree_hash = 'tree_xyz'
    │      AND status = 'succeeded'
    │    ORDER BY finished_at DESC LIMIT 1;
    │
    │    -- verify referenced derivations still exist (not GC'd)
    │    IF reuse_id IS NOT NULL THEN
    │      SELECT count(*) INTO drv_count FROM eval_attrs ea
    │      JOIN derivations d USING (drv_path)
    │      WHERE ea.eval_id = reuse_id AND ea.drv_path IS NOT NULL;
    │      IF drv_count = 0 THEN reuse_id := NULL; END IF;
    │    END IF;
    │
    │    IF reuse_id IS NOT NULL THEN
    │      INSERT INTO evaluations (…, status='skipped',
    │          skipped_reason='tree_hash_hit',
    │          reuses_eval_id=reuse_id, tree_hash='tree_xyz');
    │      -- copy eval_attrs rows from reused eval to new eval
    │      -- report success to forge immediately
    │    END IF;
    │  COMMIT;  -- serializable: aborts on concurrent conflict → retry
    │
    ├─ match found?
    │    → done, no eval work needed
    │
    └─ no match?
         → run eval normally (see Sequence below)
```

This is more general than just merge queue → main dedup. Any push with
identical source tree is skipped: merge queue landing on main, PR rebased
onto the same content, force-push with no changes. The tree hash
comparison is robust against differing commit metadata (different SHAs,
different commit messages).

## Sequence

```
webhook (push / PR)
        │
        ▼
frontend node
        │
        ├─ INSERT INTO evaluations (project_id, flake_url, commit_sha, branch, status='queued')
        ├─ cancel older evals for same project+branch (see above)
        ├─ NOTIFY evaluations
        └─ return 202 Accepted to forge webhook

                    (any evaluator node, via LISTEN evaluations or poll)
                                        │
                    ┌───────────────────┘
                    │
                    │  claim eval:
                    │  WITH claimed AS (
                    │    SELECT id, flake_url
                    │    FROM evaluations
                    │    WHERE status = 'queued'
                    │    ORDER BY id ASC LIMIT 1
                    │    FOR UPDATE SKIP LOCKED
                    │  )
                    │  UPDATE evaluations SET status='running',
                    │    claimed_by=$1, claimed_at=now()
                    │  FROM claimed WHERE evaluations.id = claimed.id
                    │  RETURNING *;
                    │
                    ├─ git fetch / nix flake fetch  (flake source → local store)
                    │
                    ├─ spawn: nix-eval-jobs --flake <url> --show-input-drvs
                    │    (sandboxed: no access to PG creds, forge tokens,
                    │     or signing keys — see security.md)
                    │
                    │  for each line of stdout (batched, see below):
                    │  ┌─────────────────────────────────────────────────────┐
                    │  │ accumulate into batch (up to 50 attrs or 2s)       │
                    │  └─────────────────────────────────────────────────────┘
                    │
                    │  flush batch:
                    │  ┌─────────────────────────────────────────────────────┐
                    │  │ filter: skip drvs already in derivations table     │
                    │  │   (SELECT drv_path FROM derivations                │
                    │  │    WHERE drv_path = ANY($1))                       │
                    │  │                                                     │
                    │  │ for new drvs only:                                  │
                    │  │   POST /api/upload-slots {drv_paths: [...]}         │
                    │  │     → batch of presigned URLs (signer skips         │
                    │  │       drvs already in S3 via HEAD check)            │
                    │  │   parallel PUT .drv NARs to S3 via presigned URLs  │
                    │  │                                                     │
                    │  │ BEGIN TRANSACTION                                   │
                    │  │  INSERT INTO derivations (batch)                    │
                    │  │    ON CONFLICT (drv_path) DO NOTHING               │
                    │  │  INSERT INTO derivation_outputs (batch)             │
                    │  │    ON CONFLICT (drv_path, output_name) DO NOTHING  │
                    │  │  INSERT INTO derivation_inputs (batch)              │
                    │  │    ON CONFLICT (referrer, input_drv) DO NOTHING    │
                    │  │  INSERT INTO eval_attrs (batch)                     │
                    │  │    ON CONFLICT (eval_id, attr) DO NOTHING          │
                    │  │  for attrs where cacheStatus != local/cached:      │
                    │  │    INSERT INTO build_jobs                             │
                    │  │      (drv_path, project_id, system, status)        │
                    │  │    VALUES ($drv, $project_id, $system, 'pending')  │
                    │  │    ON CONFLICT (drv_path) DO NOTHING  -- dedup     │
                    │  │ COMMIT                                              │
                    │  │ NOTIFY build_jobs, '{"system":"…"}'                │
                    │  └─────────────────────────────────────────────────────┘
                    │
                    ├─ on nix-eval-jobs exit 0:
                    │    UPDATE evaluations SET status='succeeded', finished_at=now()
                    │    NOTIFY evaluations
                    │
                    └─ on nix-eval-jobs crash / non-zero exit:
                         DELETE FROM eval_attrs WHERE eval_id = $1
                         -- orphaned build_jobs cleaned up by GC (no eval_attrs reference)
                         UPDATE evaluations SET status='failed',
                           error='nix-eval-jobs exited with code N',
                           finished_at=now()
                         NOTIFY evaluations
```

On eval failure (crash, timeout, or non-zero exit), all partial results
are rolled back: `eval_attrs` rows are deleted, and any `build_jobs`
rows that are now unreferenced by any active eval become orphans cleaned
up by the regular GC sweep (see [gc-and-retention.md](gc-and-retention.md)).

## Eval Timeout

The evaluator enforces a configurable timeout (server-global default,
overridable per-project in `.harmonia-ci.toml`). If `nix-eval-jobs` has
not exited within the timeout, the evaluator kills the subprocess and
marks the eval as failed. The same rollback logic applies.

The server-global default is set in the TOML config:

```toml
[eval]
timeout = "1h"
```

## Import-From-Derivation (IFD)

IFD causes `nix-eval-jobs` to build derivations mid-evaluation via the
local nix-daemon on the evaluator node. This is **allowed by default**
to maximise compatibility with existing flakes.

The evaluator passes `--allow-import-from-derivation` (true/false) to
`nix-eval-jobs` based on the project's `.harmonia-ci.toml`:

```toml
# .harmonia-ci.toml
allow-ifd = false  # attrs requiring IFD will fail to evaluate
```

When IFD is allowed, evaluator nodes need sufficient disk for the
intermediate builds. When IFD is banned, attrs that require it get an
`error` in `eval_attrs` and no `build_jobs` row is created.

## Eval Reaping

Reaping works identically to `build_jobs`: when a node's `last_seen`
expires (> 2 minutes), its claimed evaluations are reset to `queued`
and reclaimed by another node.

## What the evaluator capability requires

`nix-eval-jobs` needs:
1. **The flake source** — fetched from the forge URL over the network;
   no pre-existing local checkout needed
2. **A local Nix store** — to instantiate `.drv` files during evaluation;
   if IFD is allowed (default), the store also holds intermediate build
   outputs and needs more disk accordingly
3. **RAM** — evaluating a large flake (e.g. all of nixpkgs) uses ~4 GB
   per `--workers` subprocess; evaluator nodes should be sized accordingly
4. **The `nix-eval-jobs` binary** — not needed on builder-only or
   frontend-only nodes

The JSON output goes directly into PostgreSQL over the evaluator's DB
connection — no relay through the frontend node. The frontend node does
not need `nix-eval-jobs` installed at all.

## What nix-eval-jobs Emits vs. Where It Goes

```
nix-eval-jobs JSON field    destination
──────────────────────────  ─────────────────────────────────────
drvPath                     derivations.drv_path (PK)
name                        derivations.name
system                      derivations.system
outputs {name → path}       derivation_outputs rows
inputDrvs {drv → [outputs]} derivation_inputs rows
attr / attrPath             eval_attrs.attr
error                       eval_attrs.error
cacheStatus                 build_jobs INSERT guard (NOT stored)
neededBuilds                build_jobs INSERT guard (NOT stored)
neededSubstitutes           build_jobs INSERT guard (NOT stored)

Note: `derivations.drv_json` is NOT populated during eval. The
`nix derivation show` subprocess call (~50ms each) would add 25+
seconds for a 500-attr flake. Builders get the .drv from S3 instead.
`drv_json` is reserved for Phase 5 (PostgreSQL eval store plugin).
```
