# Forge Integration

## Supported Forges (Phase 1)

- **GitHub**: App-based webhooks (push + pull_request events), HMAC-SHA256
  verification, commit status via REST API
- **Gitea**: Same webhook structure (minor field differences), commit status
  via Gitea API

## Forge Connections

Forge connections are configured in the server TOML config. They define
how harmonia-ci authenticates with each forge instance.

```toml
[[forges]]
name = "github"
type = "github"
# GitHub App credentials
app_id = 12345
private_key_file = "/run/secrets/github-app.pem"
webhook_secret_file = "/run/secrets/github-webhook-secret"

[[forges]]
name = "gitea"
type = "gitea"
instance = "https://gitea.example.com"
token_file = "/run/secrets/gitea-token"
webhook_secret_file = "/run/secrets/gitea-webhook-secret"
```

## Project Discovery and Onboarding

Projects are **not** statically listed in the TOML config. Instead,
harmonia-ci discovers repositories from connected forges and stores
them in PostgreSQL. Admins enable projects through the web UI.

### Discovery

On startup (and periodically, every 15 minutes), each frontend node
syncs the repository list from every configured forge:

- **GitHub App**: `GET /installation/repositories` — returns all repos
  the App is installed on. Pagination handled automatically.
- **Gitea**: `GET /api/v1/user/repos` + org repos — returns all repos
  accessible to the token.

Discovered repos are upserted into the
[`repositories` table](database.md#repositories-and-projects).

### Enabling Projects

A repository becomes a CI project when an admin enables it. The
[`projects` table](database.md#repositories-and-projects) stores enabled
repos and their CI-specific settings (scheduling shares, PR trust level,
poll interval).

### Web UI: Project Management

The settings page (`/ci/settings/projects`) shows all discovered
repositories with a search bar and toggle to enable/disable CI:

```
┌─────────────────────────────────────────────────────────────┐
│  Projects                                          [Sync ↻] │
├─────────────────────────────────────────────────────────────┤
│  Search: [clan____________]                                 │
│                                                             │
│  ☑ clan-core/clan-core           github  ⚙                 │
│  ☑ clan-core/clan-infra          github  ⚙                 │
│  ☐ clan-core/website             github                     │
│  ☐ clan-core/docs                github                     │
│                                                             │
│  ☑ user/dotfiles                 gitea   ⚙                 │
│  ☐ user/experiments              gitea                      │
│  ☐ user/nix-configs              gitea                      │
│                                                             │
│  Showing 7 of 42 repositories               [Load more]    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

- **Search**: filters by repo name, owner, or forge (HTMX `hx-get`
  with debounced input, server-side `ILIKE` query)
- **Toggle (☑/☐)**: `POST /api/projects/{repo_id}/enable` or
  `/disable` — creates/updates the `projects` row
- **⚙ button**: opens per-project settings (scheduling shares,
  PR trust level) — inline HTMX form

```
┌─────────────────────────────────────────────────────────────┐
│  ⚙ clan-core/clan-core                                     │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Scheduling shares: [100    ]                               │
│  PR builds:    [☑] Trust: [collaborators ▼]                 │
│  Poll interval: [0] seconds (0 = webhook only)              │
│                                                             │
│  [Save]  [Disable project]                                  │
│                                                             │
│  ℹ attrs and retention are configured in the repository's   │
│    .harmonia-ci.toml file.                                  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Webhook Auto-Configuration

When a project is enabled, the frontend automatically registers a
webhook on the forge:

- **GitHub App**: webhooks are automatic (configured at App level)
- **Gitea**: `POST /api/v1/repos/{owner}/{repo}/hooks` with push +
  pull_request events pointing to `https://harmonia.example.com/webhook/gitea`

When a project is disabled, the Gitea webhook is removed. GitHub App
webhooks are per-installation and don't need per-repo management.

### How Webhooks Route to Projects

```
POST /webhook/{forge_name}
  → verify HMAC signature
  → extract repo full_name from payload
  → SELECT p.* FROM projects p
    JOIN repositories r ON r.id = p.repo_id
    WHERE r.forge = $1 AND r.full_name = $2 AND p.enabled = true
  → no match? return 200 (silently ignore — repo not enabled)
  → proceed with eval creation
```

### Permissions for Project Management

| Action | Who |
|---|---|
| View repo list | Admins (see below) |
| Enable/disable project | Admins |
| Edit project settings | Admins |

Admins are defined in the server config:

```toml
[auth]
admins = ["github:Mic92", "gitea:joerg"]
```

These are `{forge}:{login}` pairs checked against the signed cookie.

## Webhook Handler

```
POST /webhook/github
POST /webhook/gitea
  → verify HMAC signature (GitHub: X-Hub-Signature-256, Gitea: similar)
  → dispatch by event type:
    - push / pull_request → eval creation (below)
    - issue_comment → /ci run approval (see above)
    - installation / installation_repositories → sync repositories table
  → match (repo) to project config
  → reject if no matching project
  → extract (sha, branch, event_type)
  → classify branch (see below)
  → resolve tree hash via forge API (best-effort; NULL on failure)
  → tree hash already succeeded? skip eval (see evaluation.md)
  → INSERT INTO evaluations (project_id, flake_url, commit_sha, branch, …)
    ON CONFLICT (project_id, commit_sha, branch) DO NOTHING  -- webhook idempotency
  → if not merge queue branch: cancel older evals for same project+branch
  → NOTIFY evaluations
  → return 202 Accepted
```

### Branch Classification

| Pattern | Supersedes older evals? |
|---|---|
| `main`, `master`, configured default | Yes |
| `release/*`, configured release patterns | Yes |
| `gh-readonly-queue/*`, `staging`, `trying` | **No** — each tests a unique merge state |
| PR branches (`refs/pull/*/merge`) | Yes |

Merge queue branches must **not** supersede each other — GitHub's merge
queue tests multiple PRs in sequence, and each commit represents a
different combination that must be evaluated independently.

Compatibility with bors (`staging`/`trying` branches) is included for
Gitea deployments that use bors-ng.

## Pull Request Security

PRs can run arbitrary Nix code during evaluation and build. Trust levels
control who can trigger builds:

| `pr.trust` | Behaviour |
|---|---|
| `collaborators` | Only PRs from repo collaborators/members trigger builds automatically. External PRs require a collaborator to comment a trigger phrase (e.g. `/ci run`). |
| `all` | Any PR triggers builds. Use only for trusted public projects. |
| `none` | PRs never trigger builds (branch pushes only). |

The frontend checks PR author permissions via the forge API before
inserting an evaluation. For untrusted PRs awaiting approval, no
evaluation row is created until a collaborator approves.

### `/ci run` Approval Mechanism

When a collaborator comments `/ci run` on an untrusted PR:

1. The frontend receives an `issue_comment` webhook event (GitHub:
   `issue_comment` action `created`; Gitea: equivalent).
2. Verifies the commenter is a collaborator via `check_collaborator()`.
3. Resolves the PR's current HEAD SHA at approval time (not the SHA
   from the original PR webhook) — this prevents a race where the PR
   author pushes malicious commits after the `/ci run` comment.
4. Inserts an evaluation for that specific SHA.

The GitHub App must subscribe to `issue_comment` events (in addition
to `push` and `pull_request`). For Gitea, the webhook registers the
`issue_comment` event type.

If the PR author pushes new commits after `/ci run`, the approval
is consumed — a collaborator must comment `/ci run` again for the
new HEAD.

## Status Reporting

Two commit statuses per evaluation, matching buildbot-nix's model:

### `nix-eval` status
Reported once per evaluation. Transitions:
- **`pending`** — eval queued or running
- **`success`** — eval completed, all attributes evaluated
- **`failure`** — eval crashed or nix-eval-jobs exited non-zero
- **`error`** — eval cancelled (superseded by newer push)

Context: `harmonia-ci/nix-eval`

### `nix-build` status
Reported once per evaluation, summarising all builds. Transitions:
- **`pending`** — builds queued or in progress
- **`success`** — all builds for this eval succeeded
- **`failure`** — one or more builds failed (link to first failure)

Context: `harmonia-ci/nix-build`

The `nix-build` status is set to `pending` as soon as eval completes
(and build_jobs exist), then updated to `success`/`failure` when the
last build_job for the evaluation finishes. The frontend tracks this
by querying:

```sql
SELECT
    count(*) FILTER (WHERE bj.status IN ('failed', 'dep-failed')) AS failed,
    count(*) FILTER (WHERE bj.status NOT IN ('succeeded', 'failed', 'cancelled', 'dep-failed')) AS pending
FROM (
    SELECT DISTINCT bj.id, bj.status
    FROM eval_attrs ea
    JOIN build_jobs bj USING (drv_path)
    WHERE ea.eval_id = $1
      AND ea.drv_path IS NOT NULL
) bj;
```

Each status includes:
- `target_url` → link to evaluation/build page in Harmonia web UI
- `description` → summary (e.g. "3/42 builds complete", "2 failed")

### Eval completion detection

The frontend `LISTEN`s on the `build_jobs` channel. Terminal-state
notifications include `drv_path`, so the frontend looks up affected
evals efficiently:

```sql
-- drv_path from NOTIFY payload → which evals care?
SELECT DISTINCT eval_id FROM eval_attrs
WHERE drv_path = $1;
```

Then re-runs the aggregate query above only for those evals. When
`pending = 0`, all builds are done and the final `nix-build` status is
reported. The frontend debounces these checks (~5s) to coalesce
burst completions. Status updates to the forge API are also batched
to stay within rate limits (GitHub: 1000 statuses per SHA).

## Polling Fallback

For projects where webhooks are unreliable or unavailable, the frontend
runs a background polling loop that checks for new commits:

```
every 60 seconds:
    SELECT p.id, p.poll_interval, r.forge, r.full_name, r.default_branch
    FROM projects p
    JOIN repositories r ON r.id = p.repo_id
    WHERE p.enabled = true AND p.poll_interval > 0

    for each project (if last_polled + poll_interval < now):
        branches = {default_branch}
        for each branch:
            sha = forge.get_branch_head(repo, branch)

            -- reuses the same path as the webhook handler:
            -- tree hash dedup, UNIQUE constraint, superseding
            INSERT INTO evaluations (project_id, flake_url, commit_sha,
                branch, status='queued')
            ON CONFLICT (project_id, commit_sha, branch) DO NOTHING
```

Polling coexists safely with webhooks — the `UNIQUE(project_id,
commit_sha, branch)` constraint prevents duplicate evals. Polling is a
safety net, not a replacement. It is disabled by default
(`poll_interval = 0`) and enabled per-project in the settings UI.

## Multi-Forge Support

The webhook handler and status reporter are behind a `Forge` trait:

```rust
trait Forge {
    fn verify_webhook(&self, headers: &Headers, body: &[u8]) -> Result<WebhookEvent>;
    fn report_status(&self, repo: &str, sha: &str, status: CommitStatus) -> Result<()>;
    fn check_collaborator(&self, repo: &str, user: &str) -> Result<bool>;
}
```

GitHub and Gitea implement this trait. Adding a new forge (GitLab,
Forgejo, etc.) means implementing three methods.
