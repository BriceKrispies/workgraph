# Tracker sync (Jira)

Tracked in Workgraph as parent task WG-1667 (work items below are WG-1668 to WG-1674); progress lives
there (`workgraph task show WG-1667`), not in this document.

## Problem

At work, one Jira project holds every story, and each story names the GitHub pull request (or
several) that implements it. Those pull requests can be in any number of repositories. Workgraph
should poll that Jira instance, keep one task per issue, and treat Jira as the source of truth for
the work item while still running, attributing and verifying the work itself.

Workgraph never writes to Jira: no comments, no transitions, no field edits.

## What already fits

The data model is not one-graph-per-repository. The workspace is the boundary; tasks and projects
are many-to-many with repositories (`task_repositories`, `project_repositories`), dependencies cross
repositories, and stacks carry one base and integration branch per repository. The mapping is:

| Jira | Workgraph |
| --- | --- |
| Jira site | a workspace of its own (e.g. `work`), kept apart from `default` |
| Jira project `ABC` | a project in that workspace |
| Issue `ABC-123` | a task whose key is `ABC-123` |
| Issue's pull requests | `task_links` rows (`kind = pull_request`) and the `task_repositories` they resolve to |
| Epic / parent, sub-tasks | `parent_id` |
| `blocks` / `is blocked by` links | `task_dependencies` |

A pull request URL resolves to a repository through `repository_remotes.normalized_url`
(`integrations/git/cli.py: normalize_remote_url`): `https://github.com/org/svc-a/pull/42` ->
`github.com/org/svc-a`. A pull request in a repository that is not registered stays an unresolved
link and is reported by `workgraph tracker status`; repositories are never registered automatically.

## Model

### Ownership

An issue is split between the two systems by field, not by task:

- **Jira owns** title, description, acceptance criteria, priority, assignee, parent, the dependencies
  that came from issue links, and the tracker status.
- **Workgraph owns** claims, attempts, worktrees, commits and their attribution, verification,
  stacks, local comments and local tags.

`TaskService` rejects changes to Jira-owned fields of a tracked task unless the actor is
`system:tracker-sync`. Agents still claim, run, comment and report results on tracked tasks.

### Keys

A tracked task's key is the Jira key. Local tasks in the same workspace keep `<prefix>-<n>`.
`tasks.number` stays the workspace counter (every task still consumes one); `(workspace_id, key)` stays
unique, and Jira keys cannot collide
with local keys as long as the local prefix differs from every Jira project key (validated when a
tracker connection is configured). `resolve.task` already matches on the key string, so
`Task-ID: ABC-123` trailers, MCP calls and the CLI work unchanged. Commit attribution's message
inference learns every prefix in the workspace (WG-1674), so a Jira key in a commit or merge
message links to the task.

### Status

Workgraph's task status describes execution; Jira's describes the team's workflow. Both are kept:

- `tracker_status`: Jira's status name and its `statusCategory` (`new`, `indeterminate`, `done`),
  authoritative and read-only.
- Workgraph's `status`, constrained by it:

| Jira | Workgraph |
| --- | --- |
| category `new` | `backlog`, or `ready` when its dependencies are satisfied |
| category `indeterminate` | unchanged; agents may claim and run |
| category `done`, resolution not in `cancelled_resolutions` | `accepted`, recorded as an acceptance with reason `external:<connection>` |
| category `done`, resolution in `cancelled_resolutions` (e.g. `Won't Do`) | `cancelled` |
| `done` -> not `done` | reopened (`task.reopened`) |

Acceptance from Jira matters because dependencies are satisfied only by `accepted` blockers.
A tracked workspace sets `verification.auto_accept = false`, so Workgraph-side success stops at
`awaiting_review` and only Jira reaching `done` accepts the task.

### Tables

Migration `0008` (WG-1668):

- `external_items`: `task_id` (unique FK), `connection`, `external_id` (Jira's numeric id, stable
  across key changes), `external_key`, `url`, `tracker_status`, `tracker_status_category`,
  `remote_updated_at`, `snapshot` (raw JSON), `snapshot_hash`, `synced_at`, `missing_since`.
  Unique on `(connection, external_id)`.
- `task_links`: `task_id`, `kind` (`pull_request` first), `provider`, `url`, `normalized_url`,
  `repo_full_name`, `number`, `repository_id` (null when unresolved), `sources` (which extractors
  found it), `created_by`. Unique on `(task_id, kind, normalized_url)`.
- `tasks.external_refs` is migrated into `task_links` and dropped.

## Jira client

The client has read methods only, and a test asserts its transport issues nothing but `GET`
(WG-1669). It is an interface with one flavour per deployment:

| | Cloud (implemented) | Data Center (seam) |
| --- | --- | --- |
| Search | `GET /rest/api/3/search/jql`, `nextPageToken` | `GET /rest/api/2/search`, `startAt` |
| Auth | Basic, email + API token | Bearer personal access token |
| Description | Atlassian Document Format, flattened to text | wiki markup |
| Dev panel | `GET /rest/dev-status/latest/issue/detail?issueId=…&applicationType=GitHub&dataType=pullrequest` | same endpoint |

The dev-status endpoint is internal to Jira and undocumented; a failure there is recorded on the
connection's status and does not fail the sync, since the custom field is the second source.

## Pull request extraction

Two sources, merged and de-duplicated by normalised URL, each link remembering which source found it
(WG-1670):

1. **The configured custom field** (`pr_fields`). Its value may be a string, a list, or ADF; every
   GitHub pull request URL in it is taken.
2. **The GitHub for Jira development panel**, from the dev-status endpoint.

## Sync

`TrackerSyncService` (WG-1672) follows the architecture rule: fetch outside any write transaction,
then write each page in a short transaction.

- **Incremental.** JQL `project in (<projects>) AND updated >= "<cursor - overlap>" ORDER BY updated
  ASC`; the cursor is a `system_cursors` row per connection. An issue is applied only when its
  `snapshot_hash` changed. Each application emits `task.synced` with actor `system:tracker-sync`.
- **Full.** Lists every key in scope; issues no longer returned (deleted, moved out of the projects)
  get `missing_since` and their tasks are archived after `missing_grace_seconds`.
- **Order.** Parents and blockers outside the fetched page are fetched by key; an issue outside the
  configured projects becomes a link in the snapshot, not a task.

The `tracker_sync` job runs with resource key `tracker:<name>`, and the maintenance leader enqueues
it per connection every `poll_interval_seconds` (WG-1673). Polling rather than webhooks, because the
server binds to loopback only.

## Configuration

```toml
[trackers.work-jira]
type = "jira"
flavour = "cloud"                 # "data_center" once implemented
base_url = "https://acme.atlassian.net"
workspace = "work"
projects = ["ABC"]                # Jira project keys; each maps to a Workgraph project of the same slug
email_env = "WORKGRAPH_JIRA_EMAIL"
token_env = "WORKGRAPH_JIRA_TOKEN"
poll_interval_seconds = 300
full_sync_interval_seconds = 86400
overlap_seconds = 120
missing_grace_seconds = 86400
pr_fields = ["customfield_10042"]
dev_panel = true
cancelled_resolutions = ["Won't Do", "Duplicate"]
```

Credentials come only from the named environment variables.

## Not in scope

Writing to Jira in any form. Fetching pull request state or merge commits from GitHub (a later
attribution source, `pull_request`). Sprints, boards and Jira user mapping.
