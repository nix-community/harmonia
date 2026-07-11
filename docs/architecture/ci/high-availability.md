# High Availability

## Job Claiming (no races, no external broker)

The full claim query (with DAG readiness, fairness, and priority) is in
[build-protocol.md](build-protocol.md#work-stealing-via-postgresql).
The key primitive is `FOR UPDATE SKIP LOCKED` — multiple builders
race to claim jobs, and PostgreSQL guarantees exactly one winner per
row with no blocking.

## Heartbeat and Failover

Liveness is tracked **per node**, not per job. Each node updates
`nodes.last_seen` every 30 seconds. A node absent for > 2 minutes is
considered dead, and all its claimed jobs are reclaimed:

The reaper is guarded by an advisory lock so that only one frontend
node runs it at a time, preventing double-increment of `retry_count`:

```sql
-- Only one node runs the reaper at a time
SELECT pg_try_advisory_lock(hashtext('reaper'));
-- returns false on other nodes → skip this cycle

-- Reset stale builds from dead nodes (must rebuild from scratch)
UPDATE build_jobs SET
    status      = 'pending',
    claimed_by  = NULL,
    claimed_at  = NULL,
    retry_count = retry_count + 1
WHERE status    = 'building'
  AND claimed_by IN (
      SELECT id FROM nodes WHERE last_seen < now() - interval '2 minutes'
  )
  AND retry_count < 3;

-- Stale builds that exhausted retries → permanent failure
UPDATE build_jobs SET status = 'failed'
WHERE status    = 'building'
  AND claimed_by IN (
      SELECT id FROM nodes WHERE last_seen < now() - interval '2 minutes'
  )
  AND retry_count >= 3;

-- Reset stale uploads from dead nodes back to pending (must rebuild —
-- the dead node's local store with the build outputs is inaccessible).
-- If NARs were already uploaded to S3, the signer's HEAD check will
-- skip them on the next attempt, and only the narinfo write remains.
UPDATE build_jobs SET
    status      = 'pending',
    claimed_by  = NULL,
    claimed_at  = NULL,
    retry_count = retry_count + 1
WHERE status   = 'uploading'
  AND claimed_by IN (
      SELECT id FROM nodes WHERE last_seen < now() - interval '2 minutes'
  )
  AND retry_count < 3;

UPDATE build_jobs SET status = 'failed'
WHERE status   = 'uploading'
  AND claimed_by IN (
      SELECT id FROM nodes WHERE last_seen < now() - interval '2 minutes'
  )
  AND retry_count >= 3;
```

Stuck builds (infinite loop, hung process) are handled by the builder
itself via wall-clock and max-silent timeouts — see
[build-protocol.md](build-protocol.md#build-timeout). The heartbeat
reaper only handles node death, not stuck builds on live nodes.

This gives HA without any external message broker (no WAMP/crossbar.io,
no RabbitMQ). Multiple coordinators can run against the same PostgreSQL
instance.

## Status Reporting Deduplication

Multiple frontend nodes all `LISTEN build_jobs` and react to the same
completion notifications. To prevent duplicate forge status API calls,
the frontend uses optimistic locking before reporting:

```sql
UPDATE evaluations
SET last_status = $new_status, last_status_at = now()
WHERE id = $eval_id AND last_status IS DISTINCT FROM $new_status
RETURNING id;
```

Only the node that wins the UPDATE reports to the forge. Other nodes
see zero rows returned and skip. This adds two columns to `evaluations`
(`last_status TEXT`, `last_status_at TIMESTAMPTZ`) but avoids advisory
locks per eval.

## LISTEN/NOTIFY for Low Latency

Workers are notified immediately when a matching job is enqueued, avoiding
the polling interval cost:

```sql
-- On INSERT into build_jobs (wake builders):
NOTIFY build_jobs, '{"system": "x86_64-linux"}';

-- On status change to terminal state (wake frontend for forge status):
NOTIFY build_jobs, '{"drv_path": "/nix/store/...", "status": "succeeded"}';

-- Workers do:
LISTEN build_jobs;
-- Block until woken, then run the claim query above
```

## Node Registration

Each node registers its own capabilities in PostgreSQL on startup and
maintains a heartbeat. No static node list is needed anywhere. See
[database.md](database.md#node-registration) for the full schema.

Builders discover signer endpoints at runtime:
```sql
SELECT endpoint FROM nodes
WHERE 'signer' = ANY(capabilities)
  AND last_seen > now() - interval '2 minutes';
```

Nodes update `last_seen` every 30 seconds. Nodes absent for > 2 minutes
are considered offline; their claimed `evaluations` and `build_jobs` rows
are reclaimed by the heartbeat reaper.

## Node Draining

To gracefully remove a node (maintenance, upgrade), set it to draining:

```sql
UPDATE nodes SET draining = true WHERE id = $1;
```

The draining node itself stops calling the claim query — it only runs
its heartbeat loop and waits for in-flight jobs to complete, then exits
cleanly. No claim query changes needed.

Draining can be triggered two ways:

1. **Locally**: `harmonia-ci drain` (or SIGTERM with graceful shutdown)
2. **Remotely**: `POST /api/nodes/{id}/drain` — sets `draining = true`
   in the DB. The node checks `SELECT draining FROM nodes WHERE id = $me`
   on each heartbeat (every 30s) and enters drain mode when set.

The web UI nodes page shows draining nodes with a distinct status badge.
