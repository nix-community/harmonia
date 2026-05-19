# Security Model

## Trust Zones

The system has four trust zones with decreasing trust levels:

```
┌─────────────────────────────────────────────────────────────────┐
│  Zone 1: Signer  (highest trust)                                │
│    - Holds Ed25519 signing key + S3 credentials                 │
│    - Can forge narinfo (defines what's in the cache)            │
│    - Can write arbitrary objects to S3                          │
│    - MUST NOT run arbitrary code (no eval, no build)            │
├─────────────────────────────────────────────────────────────────┤
│  Zone 2: Frontend  (high trust)                                 │
│    - Holds forge credentials (GitHub App key, Gitea token)      │
│    - Holds cookie signing secret (HMAC-SHA256)                  │
│    - Holds webhook secrets                                      │
│    - Has write access to all PostgreSQL tables                  │
│    - MUST NOT run arbitrary code                                │
├─────────────────────────────────────────────────────────────────┤
│  Zone 3: Evaluator  (medium trust)                              │
│    - Runs nix-eval-jobs (evaluates arbitrary Nix expressions)   │
│    - nix-eval-jobs subprocess is sandboxed: no access to PG     │
│      credentials, forge tokens, or signing keys                 │
│    - Evaluator binary has write access to PG (derivations, etc) │
│    - No S3 credentials (gets presigned URLs from signer)        │
│    - No signing key                                             │
│    - IFD may cause builds on the evaluator's local nix-daemon   │
├─────────────────────────────────────────────────────────────────┤
│  Zone 4: Builder  (lowest trust)                                │
│    - Runs arbitrary derivations via harmonia-daemon              │
│    - Has write access to PostgreSQL (build_jobs, build_log_tails)│
│    - No S3 credentials (gets presigned URLs from signer)        │
│    - No signing key, no forge credentials                       │
│    - Compromised builder can: waste compute, upload wrong NARs  │
│    - Compromised builder CANNOT: forge narinfo, write arbitrary │
│      S3 objects, push fake commit statuses, sign cache entries  │
└─────────────────────────────────────────────────────────────────┘
```

In small deployments all zones coexist on one node (convenience over
isolation). In large deployments, builders are the most exposed attack
surface and should be isolated from signing key material.

## Threat Analysis

### T1: Malicious PR code execution

**Threat**: An untrusted PR submits Nix code that exfiltrates secrets,
mines crypto, or attacks the network during eval or build.

**Mitigations**:
- **PR trust levels** (`collaborators` / `all` / `none`) gate which
  PRs trigger evaluation. Default: `collaborators` — only repo members
  get automatic builds. External PRs need explicit `/ci run` approval.
- **Evaluator sandboxing** — `nix-eval-jobs` is spawned without access
  to secrets (PG credentials, forge tokens, signing keys). See
  [Evaluator Sandboxing](#evaluator-sandboxing) below.
- **Nix sandbox** (`sandbox = true`) is recommended on all nodes to
  prevent network access and filesystem escape during builds/IFD.
- **IFD during eval** — projects can ban IFD via `.harmonia-ci.toml`
  (`allow-ifd = false`).
- **Eval timeout** kills runaway `nix-eval-jobs` processes.
- **Build timeout** (4h) kills runaway builds.
- **Resource limits**: evaluators bound RAM (nix-eval-jobs `--max-memory-size`),
  builders bound disk via Nix's `min-free` / `max-free` settings.

**Residual risk**: Builds can still consume CPU/disk/time within the
timeout. Build-time secrets are explicitly deferred to a later phase.

### T2: Webhook forgery

**Threat**: An attacker sends fake webhook payloads to trigger builds
of arbitrary commits or poison eval results.

**Mitigations**:
- **HMAC-SHA256 verification** on every webhook. GitHub:
  `X-Hub-Signature-256` header. Gitea: equivalent header. The webhook
  secret is per-forge, stored in a file referenced by config
  (`webhook_secret_file`). Requests with missing or invalid signatures
  are rejected with 401.
- **Webhook idempotency** (`UNIQUE(project_id, commit_sha, branch)`)
  prevents duplicate processing even if an attacker replays a valid
  signed webhook.
- The webhook handler only processes repos that are enabled as projects.
  Unknown repos are silently ignored (200, no action).

### T3: Compromised builder forges cache content

**Threat**: A compromised builder uploads a tampered NAR to S3 and
tries to make it appear as a legitimate build output.

**Mitigations**:
- **Builders cannot write narinfo** — only the signer writes `.narinfo`
  files to S3. Builders receive presigned PUT URLs scoped to specific
  S3 keys (`nar/<hash>.nar.zst`) with a fixed `Content-Length` and
  `x-amz-checksum-sha256`. They cannot write to arbitrary keys.
- **S3 checksum verification** — the presigned URL includes
  `x-amz-checksum-sha256` matching the hash the builder reported. If
  the uploaded bytes don't match, S3 rejects the PUT.
- **Signer validates metadata** — the signer verifies that the
  `drv_path` in the build-complete request exists in the database and
  the job is in the expected state before signing.
- **Canonical hash (first writer wins)** — `derivation_outputs.nar_hash`
  is set only once (`WHERE nar_hash IS NULL`). A second builder building
  the same drv cannot overwrite the canonical hash.

**Residual risk**: A compromised builder can upload a valid-but-wrong NAR
(the Nix sandbox, if enabled, is the primary defense against build
output tampering).
Mismatches are logged as builder warnings, providing post-hoc visibility.

### T4: Presigned URL abuse

**Threat**: A builder leaks or reuses presigned URLs to write unexpected
objects to S3.

**Mitigations**:
- **Short expiry** (15 minutes). URLs become useless quickly.
- **Scoped to specific key** — the presigned URL allows PUT to exactly
  one S3 key (e.g. `nar/abc123.nar.zst`). Cannot write to other keys.
- **Content-Length enforced** — S3 rejects uploads that don't match the
  declared size.
- **Checksum enforced** — S3 rejects uploads that don't match the
  declared SHA256.
- Even if a URL leaks, the attacker can only replace that specific NAR
  with content matching the declared size and hash (i.e., the same
  content).

### T5: Cookie forgery / session hijacking

**Threat**: An attacker forges an authentication cookie to impersonate
a user and trigger rebuilds or cancel builds.

**Mitigations**:
- **HMAC-SHA256 signed cookies** — the cookie payload
  (`{user_id, forge, login, expires_at}`) is signed with a server
  secret. Forgery requires the secret.
- **Cookie attributes**: `Secure` (HTTPS only), `HttpOnly` (no JS
  access), `SameSite=Lax` (blocks cross-origin POST — primary CSRF
  defense), `Path=/ci/`.
- **24-hour expiry** — short window limits the impact of a stolen cookie.
- **No server-side sessions** — no session table to attack. Revocation
  is by rotating the server secret (invalidates all cookies).
- **Action-level permission checks** — even with a valid cookie, rebuild
  and cancel actions verify forge permissions (PR author, repo writer)
  via the forge API. A stolen cookie from a user without repo access
  cannot trigger builds.

### T6: PostgreSQL as shared state

**Threat**: A compromised evaluator or builder with DB write access
modifies CI state (e.g., marks builds as succeeded, changes priorities).

**Mitigations**:
- **Principle of least privilege** — in a hardened deployment, each
  node role uses a different PostgreSQL role with restricted grants:

| Node role | PostgreSQL grants |
|---|---|
| Frontend | Full read/write (inserts evals, updates status, manages projects) |
| Evaluator | INSERT on derivations, derivation_outputs, derivation_inputs, eval_attrs, build_jobs. UPDATE on evaluations (own claimed rows). SELECT on all. |
| Builder | UPDATE on build_jobs (own claimed rows). INSERT/UPDATE/DELETE on build_log_tails. SELECT on all. |
| Signer | UPDATE on build_jobs (status transitions). UPDATE on derivation_outputs (nar_hash). SELECT on all. |

  Row-level security (RLS) can further restrict UPDATE to rows where
  `claimed_by = current_setting('app.node_id')`, but this adds
  complexity and is not required for Phase 1.

**Residual risk**: In single-node or small deployments, all roles share
one PG user. The role separation is a deployment hardening step, not an
architectural requirement.

### T7: S3 bucket access

**Threat**: Direct S3 access bypasses narinfo signatures.

**Mitigations**:
- **S3 bucket policy** should restrict direct PutObject to the signer
  node's IAM role. Builders only write via presigned URLs (which the
  signer's IAM role generates).
- **Nix clients verify signatures** — even if an attacker writes a
  tampered NAR directly to S3, Nix clients reject it because the
  narinfo signature won't match the content. The signing key is only
  on signer nodes.
- The S3 bucket should NOT have public write access. Public read is
  acceptable (it's a binary cache).

### T8: Eval-time network exfiltration

**Threat**: A malicious Nix expression uses `builtins.fetchurl` or IFD
during evaluation to exfiltrate data (environment info, store contents)
to an attacker-controlled server. Nix's build sandbox blocks network
during builds, but **evaluation is not sandboxed by default** — pure
Nix evaluation has network access for fetchers.

**Mitigations**:
- **Evaluator network namespace** — in hardened deployments, run the
  `nix-eval-jobs` subprocess in a network namespace that only allows
  connections to the forge (for `git fetch`) and the binary cache (for
  substitution). All other outbound traffic is blocked.
- **`allowed-uris` in `nix.conf`** — restricts `builtins.fetchurl` and
  `builtins.fetchTarball` to allowlisted URL prefixes. Recommended on
  evaluator nodes.
- **PR trust levels** — untrusted PRs don't trigger eval at all (the
  primary defense). Only collaborator PRs or `/ci run`-approved PRs
  are evaluated.

**Residual risk**: If `allowed-uris` is not configured and the evaluator
runs without a restricted network namespace, a malicious expression from
a trusted collaborator can contact arbitrary hosts during evaluation.
Operators should configure `allowed-uris` on evaluator nodes.

## Secret Material Summary

| Secret | Held by | Storage | Purpose |
|---|---|---|---|
| Ed25519 signing key | Signer | File (`signing_key_file`) | Signs narinfo fingerprints |
| S3 credentials | Signer | Environment / IAM role | Direct S3 writes (narinfo), presigned URL generation |
| GitHub App private key | Frontend | File (`private_key_file`) | Authenticates as GitHub App (API, webhook verification) |
| Gitea API token | Frontend | File (`token_file`) | Authenticates to Gitea API |
| Webhook secrets | Frontend | File (`webhook_secret_file`) | HMAC verification of incoming webhooks |
| Cookie signing secret | Frontend | Config (`[auth] secret_file`) | HMAC-SHA256 for signed authentication cookies |
| PostgreSQL credentials | All nodes | Connection string / env | Database access |

No secrets are passed to Nix builds. Build-time secret support (vault
integration, sandbox-mounted credentials) is explicitly deferred.

## Evaluator Sandboxing

`nix-eval-jobs` evaluates arbitrary Nix expressions — including from
untrusted PRs. The evaluator process is sandboxed to limit what
malicious Nix code can do during evaluation:

- **Landlock / seccomp** (Linux): the evaluator spawns `nix-eval-jobs`
  inside a restricted filesystem namespace. Allowed paths:
  - `/nix/store` (read-only)
  - The Nix state directory (`/nix/var/nix`) for the local store
  - A temporary working directory (per-eval tmpdir)
  - Network access to the forge (git fetch) and S3 (substitution)
- **No access to** the PostgreSQL connection string, forge credentials,
  signing keys, or any other secret material. The evaluator binary
  holds these — the `nix-eval-jobs` subprocess does not inherit them.
- **Resource limits**: `nix-eval-jobs` is spawned with cgroup limits
  (memory, CPU) or ulimits. The eval timeout kills the process if it
  exceeds the configured duration.

IFD (import-from-derivation) builds during eval inherit sandbox
restrictions via the local nix-daemon's `sandbox = true` setting.

The Nix sandbox (`sandbox = true` in `nix.conf`) is recommended on
all nodes but is the operator's responsibility — the CI system does
not enforce or verify it.
