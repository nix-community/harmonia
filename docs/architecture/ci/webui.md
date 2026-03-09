# Web UI

## Technology

HTMX + server-rendered HTML. No JavaScript framework, no SPA. The
frontend capability serves pages via actix-web with HTML templates.
Live updates use SSE (Server-Sent Events) via HTMX's `hx-ext="sse"`.

## Pages

### Dashboard (`/ci/`)

Overview of all projects and their current state.

```
┌─────────────────────────────────────────────────────────────┐
│  Harmonia CI                                                │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  clan-core                                          ● main │
│    ✓ eval #142  ·  38/42 builds ✓  ·  4 building    2m ago │
│                                                             │
│  nixpkgs-custom                                     ● main │
│    ✗ eval #87   ·  120/135 ✓  ·  3 failed           5m ago │
│                                                             │
│  dotfiles                                           ● main │
│    ✓ eval #23   ·  all 8 builds ✓                  12m ago │
│                                                             │
│  Queue: 14 pending · 6 building · 2 uploading              │
│  Nodes: 3 online (build-x86-01, build-x86-02, eval-01)    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

Each project row links to the project page. The queue/node summary
at the bottom is a global status bar. The page auto-updates via SSE.

### Project Page (`/ci/{project}/`)

List of recent evaluations for a project, grouped by branch.

```
┌─────────────────────────────────────────────────────────────┐
│  clan-core                                                  │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  main                                                       │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ #142  abc1234  "feat: add widget"     38/42 ✓  ⏳ 4  │   │
│  │ #141  def5678  "fix: memory leak"     42/42 ✓       │   │
│  │ #140  ghi9012  "refactor: cleanup"    42/42 ✓       │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                             │
│  Pull Requests                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ PR #53  jkl3456  "add new feature"    12/15 ✓  ✗ 3  │   │
│  │ PR #51  mno7890  "update deps"        8/8 ✓         │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Evaluation Page (`/ci/{project}/eval/{id}`)

All attributes from one evaluation with their build status.

```
┌─────────────────────────────────────────────────────────────┐
│  clan-core · eval #142 · abc1234 · main                     │
│  "feat: add widget"                                         │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Filter: [All] [Failed] [Building] [Pending] [Succeeded]   │
│                                                             │
│  checks.x86_64-linux.treefmt          ✓   3s    build-01   │
│  checks.x86_64-linux.clippy           ✓  45s    build-02   │
│  checks.x86_64-linux.nixos-test       ⏳  2m    build-01   │
│  checks.aarch64-linux.treefmt         ✓   4s    build-03   │
│  checks.aarch64-linux.nixos-test      ✗  12m    build-03   │
│  packages.x86_64-linux.harmonia       ✓  1m     build-02   │
│  packages.x86_64-linux.harmonia-ci    ⏳  —     (pending)   │
│  ...                                                        │
│                                                             │
│  Summary: 38 succeeded · 1 failed · 2 building · 1 pending │
│                        [Re-evaluate] [Rebuild failed] [Cancel]  │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

Each row links to the build page. Rows update live via SSE as builds
complete. Duration shown is wall-clock time. The filter bar uses
HTMX `hx-get` to re-render the table server-side.

### Build Page (`/ci/{project}/build/{id}`)

Single build details with live log streaming.

```
┌─────────────────────────────────────────────────────────────┐
│  clan-core · checks.aarch64-linux.nixos-test                │
│  eval #142 · abc1234 · main                                 │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Status:    ✗ failed (exit code 1)                          │
│  Duration:  12m 34s                                         │
│  Builder:   build-03 (aarch64-linux)                        │
│  Derivation: /nix/store/abc...-nixos-test.drv               │
│  Attempt:   1/3                                             │
│                                                             │
│  [Rebuild]  [Cancel]                                        │
│  (rebuilds this single build with priority = 100)           │
│                                                             │
│  ┌─ Build Log ──────────────────────────────────────────┐   │
│  │ building '/nix/store/abc...-nixos-test.drv'...       │   │
│  │ unpacking sources                                    │   │
│  │ building                                             │   │
│  │ running NixOS test 'my-test'...                      │   │
│  │ machine: start                                       │   │
│  │ machine: waiting for unit default.target              │   │
│  │ ...                                                  │   │
│  │ FAIL: test assertion failed at line 42               │   │
│  │                                              ▼ auto  │   │
│  └──────────────────────────────────────────────────────┘   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

**Log streaming**: while the build is in progress (`status = building`),
the log panel connects to `/api/builds/{id}/log` SSE endpoint (see
[build-protocol.md](build-protocol.md#frontend-sse-endpoint)). The
frontend streams `log_tail` content as it updates. When the build
finishes, the full log is loaded from S3 via harmonia-cache.

**Rebuild button**: triggers `POST /api/builds/{id}/rebuild` which
resets the build_job to `pending` with `priority = 100` (manual rebuild).

### Nodes Page (`/ci/nodes`)

Fleet status overview.

```
┌─────────────────────────────────────────────────────────────┐
│  Nodes                                                      │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  build-x86-01   x86_64-linux     4/8 jobs   ● online       │
│  build-x86-02   x86_64-linux     7/8 jobs   ● online       │
│  build-arm-01   aarch64-linux    2/4 jobs   ● online       │
│  eval-01        evaluator        1/2 evals  ● online       │
│  signer-01      signer+frontend  —          ● online       │
│  build-x86-03   x86_64-linux     0/8 jobs   ○ offline 5m   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

## URL Structure

```
/ci/                              dashboard
/ci/{project}/                    project page (recent evals by branch)
/ci/{project}/eval/{id}           evaluation detail (all attrs + status)
/ci/{project}/build/{id}          build detail + live log
/ci/nodes                         fleet status
/ci/queue                         global build queue (all projects)
/ci/settings/projects             project discovery + onboarding (admin)
/ci/settings/projects/{id}        per-project settings (admin)
```

## SSE Endpoints (internal, consumed by HTMX)

| Endpoint | LISTEN channel | Sends |
|---|---|---|
| `/api/events/dashboard` | `evaluations`, `build_jobs` | Re-rendered project rows |
| `/api/events/eval/{id}` | `build_jobs` | Re-rendered attr rows for this eval |
| `/api/builds/{id}/log` | `build_log` | Log tail chunks |

All SSE endpoints use PostgreSQL `LISTEN/NOTIFY`. The frontend holds
one PostgreSQL connection **per channel** (not per client) — e.g., one
`LISTEN build_log` connection is shared across all SSE clients watching
any build log. Notifications are fanned out in-process to matching SSE
clients. This keeps PG connection count bounded by the number of
distinct channels, not the number of browser tabs. Notifications are
forwarded as SSE `data:` frames containing HTMX-swappable HTML fragments
(`hx-swap-oob`).

## Pagination

All list views use **keyset (cursor) pagination** — no OFFSET, which
degrades as page depth increases. The cursor is the `id` of the last
item on the current page:

```sql
SELECT * FROM evaluations
WHERE project_id = $1 AND id < $cursor
ORDER BY id DESC
LIMIT 25;
```

HTMX "load more" button at the bottom of each list:

```html
<button hx-get="/ci/{project}/?before={last_id}"
        hx-target="#eval-list"
        hx-swap="beforeend">
  Load more
</button>
```

The server returns the next page as HTML rows. No JavaScript pagination
logic. The URL query parameter `?before=` makes pages bookmarkable and
linkable.

The build queue page (`/ci/queue`) uses the same pattern, sorted by
`priority DESC, created_at ASC` with a composite cursor.

## Authentication and Authorization

### Login

Users authenticate via their forge account. No separate user database.

- **GitHub**: GitHub App user authorization flow (uses the same App as
  webhooks — tighter permissions, installation-scoped)
- **Gitea**: OAuth2 App flow

On successful OAuth callback, the frontend issues a **signed cookie**
containing `{user_id, forge, login, expires_at}`, signed
with HMAC-SHA256 using a server secret (`[auth] secret_file` in config).
No server-side session state. Cookie expiry: 24 hours. Users
re-authenticate via OAuth after expiry. Rotating the secret invalidates
all active cookies (see [security.md](security.md#t5-cookie-forgery--session-hijacking)).

### Project Visibility

Mirrors forge permissions — if you can see the repo on GitHub/Gitea,
you can see the project in Harmonia:

| Project config | Unauthenticated | Authenticated user |
|---|---|---|
| Public repo | ✓ visible | ✓ visible |
| Private repo | ✗ hidden | ✓ if user has repo access on forge |

The frontend checks repo access via the forge API on first visit to a
private project and caches the result in the signed cookie (or a
short-lived in-memory cache, ~5 min TTL) to avoid hitting the forge
API on every page load.

Public project pages (dashboard, eval, build, logs) are accessible
without login. The login button appears in the nav bar but is not
required for public content.

### Action Permissions

| Action | Who can do it |
|---|---|
| View public project | Anyone |
| View private project | Users with read access to repo on forge |
| Rebuild a PR build | PR author (owns the branch) |
| Rebuild a branch build | Users with write/push access to repo on forge |
| Rebuild any build | Harmonia-ci admins (configured in TOML) |

Rebuild endpoints check forge permissions before resetting jobs:

```
POST /api/builds/{id}/rebuild
  → verify signed cookie (logged in?)
  → look up build → eval → project → repo
  → if PR build: check cookie.login == PR author
  → if branch build: check write access via forge API
  → if neither: reject 403
  → reset build_job to pending with priority = 100

POST /api/evals/{id}/rebuild-failed
  → same permission check
  → UPDATE build_jobs SET status = 'pending',
      priority = 100, retry_count = 0,
      claimed_by = NULL, claimed_at = NULL,
      started_at = NULL, finished_at = NULL,
      exit_code = NULL
    WHERE status IN ('failed', 'dep-failed')
      AND drv_path IN (
        SELECT drv_path FROM eval_attrs
        WHERE eval_id = $1 AND drv_path IS NOT NULL
      )
  → NOTIFY build_jobs
  → re-report nix-build status as pending to forge
```

### Cancel

Cancel applies to both evaluations and individual builds via a single
button. Cancelling an eval also cancels its pending builds.

```
POST /api/evals/{id}/re-evaluate
  → verify signed cookie
  → same permission check as rebuild
  → old eval must be in terminal state (failed/cancelled/succeeded)
  → DELETE FROM evaluations WHERE id = $1
    (cascades to eval_attrs; orphaned build_jobs cleaned by GC)
  → INSERT INTO evaluations (project_id, flake_url, commit_sha, branch,
      status='queued')  -- UNIQUE constraint satisfied since old row deleted
  → NOTIFY evaluations
  → re-report nix-eval status as pending to forge

POST /api/evals/{id}/cancel
  → verify signed cookie
  → same permission check as rebuild (PR author or branch writer)
  → UPDATE evaluations SET status='cancelled', finished_at=now()
    WHERE id = $1 AND status IN ('queued', 'running')
  → cancel pending build_jobs exclusively owned by this eval
    (same superseding logic as evaluation.md)
  → if eval was running: evaluator detects cancelled status on next
    periodic check and kills nix-eval-jobs

POST /api/builds/{id}/cancel
  → verify signed cookie
  → same permission check as rebuild
  → UPDATE build_jobs SET status='cancelled'
    WHERE id = $1 AND status IN ('pending', 'building')
  → if build was running: builder detects failed status on next
    heartbeat check and kills harmonia-daemon build
```

### Unauthenticated API

Build logs, eval results, and narinfo for public projects are served
without authentication. This ensures `nix log` and `nix build
--substituters` work without credentials.

## Design Principles

- **No client-side state** — all rendering is server-side. HTMX swaps
  HTML fragments. No JSON APIs consumed by JavaScript.
- **Progressive enhancement** — pages work without JavaScript (static
  HTML, manual refresh). SSE adds live updates.
- **Minimal dependencies** — no npm, no bundler, no node_modules.
  HTMX is a single `<script>` tag (~14 KB gzipped).
- **Forge links** — every page that shows a commit SHA links back to the
  forge (GitHub/Gitea commit page). Every build status on the forge
  links to the corresponding Harmonia build page.
