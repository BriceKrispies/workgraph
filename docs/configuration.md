# Configuration

Configuration is TOML, validated by `crates/wg-config`.
Unknown keys are rejected.

- `config.toml` - global settings.
- `workspaces/<slug>.toml` - per-workspace overrides (`llm`, `agents`, `execution`, `jobs`),
  deep-merged over the global file.

```sh
workgraph config path
workgraph config show
workgraph config validate
workgraph config migrate
workgraph config set execution.max_concurrent_runs 3
workgraph config set llm.classification.model qwen2.5:14b-instruct
workgraph config set execution.enabled true --workspace acme
```

`config set` parses the value as JSON when possible and validates the whole document before
writing it atomically.

## Reference

```toml
schema_version = 2
default_workspace = "acme"

[server]
host = "127.0.0.1"          # loopback by default
port = 8765
allow_non_loopback = false  # binding elsewhere also needs --allow-remote
lan = true                  # also serve on the local-network address for paired devices
# lan_address = "192.168.1.20"       # override the detected address
# lan_networks = ["192.168.1.0/24"]  # narrow the allowed clients (default: RFC 1918 ranges)

[llm]
auto_classify = true        # enqueue classification on create / material description change
auto_embed = true

[llm.providers.ollama]
type = "ollama"
base_url = "http://127.0.0.1:11434"
timeout_seconds = 120
max_retries = 2
backoff_seconds = 1.0
# api_key_env = "OLLAMA_API_KEY"   # credentials only ever come from the environment

[llm.providers.heuristic]
type = "heuristic"

# [llm.providers.lmstudio]
# type = "openai_compatible"
# base_url = "http://127.0.0.1:1234"

[llm.classification]
provider = "ollama"
model = "qwen2.5:7b-instruct"

[llm.embeddings]
provider = "ollama"
model = "nomic-embed-text"

[llm.summarization]
provider = "ollama"
model = "qwen2.5:7b-instruct"

[agents.claude_code]
type = "claude_code"
executable = "claude"
permission_mode = "acceptEdits"   # acceptEdits | auto | bypassPermissions | manual | dontAsk | plan
allowed_tools = []
timeout_seconds = 3600
# model = "sonnet"
# max_budget_usd = 5.0

[agents.my-agent]
type = "command"
executable = "C:/tools/my-agent.exe"
args = ["--manifest", "{manifest}", "--prompt", "{prompt_file}"]
timeout_seconds = 1800
supports_usage = true

[execution]
enabled = false                # agent code execution is opt-in
default_agent = "claude_code"
max_concurrent_runs = 2
max_attempts_per_task = 3      # then the task is tagged needs-review (escalation)
auto_schedule = false          # scheduler starts auto_execute tasks when true
isolation = "worktree"         # or "in_place" (exclusive checkout lease)
allow_push = false
allow_merge = false
# worktree_root = "D:/wt"      # attempt worktrees as <root>/<task-key>-<n> (default: <data>/worktrees/<attempt id>)

[jobs]
worker_concurrency = 4
gpu_concurrency = 1            # concurrent model jobs (resource key "gpu")
per_agent_concurrency = 2      # resource key "agent:<name>"
per_repository_concurrency = 1 # resource key "repo:<id>" (git jobs)
verify_concurrency = 1         # resource key "verify:<id>": heavy verification jobs per repository
lease_seconds = 60
default_max_attempts = 5

[stacks]
auto_accept = true             # land a stack as soon as its full verification passes (no parent acceptance)

[verification]
auto_accept = true             # accept a task as soon as its full verification passes (no `task accept`)

[retention]
telemetry_payload_days = 30    # raw hook payloads are compacted after this
run_log_days = 90              # agent/verification log files are purged after this

[claude]
capture_tool_payloads = false  # tool inputs/outputs are summarised (shape only) by default
max_payload_chars = 4000

[automation]
max_depth = 3                  # global recursion limit for rule chains

[trackers.work-jira]
type = "jira"
flavour = "cloud"                 # "data_center" once implemented
base_url = "https://acme.atlassian.net"
workspace = "work"
projects = ["ABC"]                # Jira project keys; each maps to a Workgraph project of the same slug
auth = "basic"                    # Cloud: "basic" (email + API token) or "bearer" (OAuth 2.0 access token)
email_env = "WORKGRAPH_JIRA_EMAIL"
token_env = "WORKGRAPH_JIRA_TOKEN"  # or token_command = ["my-oauth-helper", "token"]; exactly one
token_command_timeout_seconds = 60
poll_interval_seconds = 300
full_sync_interval_seconds = 86400
overlap_seconds = 120
missing_grace_seconds = 86400
pr_fields = ["customfield_10042"]
pr_default_repos = { customfield_10042 = "https://github.com/acme/service" }
# assignee_field = "customfield_10100"  # a user field to read instead of Jira's assignee
# jql = "assignee = currentUser()"      # ANDed onto every search; no ORDER BY
min_request_interval_seconds = 0.0      # smallest gap between requests, up to 60
dev_panel = true
cancelled_resolutions = ["Won't Do", "Duplicate"]
```

Workspace settings stored in the database (not secrets) are managed with
`workgraph workspace set <slug> --allowed-root DIR`, which restricts repository registration
to paths below the given directories.

`[stacks] auto_accept` is read per workspace: a workspace file can set a `[stacks]` table of its own to
override the global value. With it on, a stack whose full verification passes lands on the default
branch without anyone accepting the parent (see [Git](git.md#stack-landing)). It is on by default, and
it is the only setting in the `stacks` table. Project-level overrides do not exist yet, as for the
other settings.

`[verification] auto_accept` is read the same way (a workspace file can set its own `[verification]`
table). With it on, a task that is not a stack item and not a stack parent is accepted as soon as its
full verification run passes all required checks, with the reason `verification.auto_accept`. A
fast-tier-only run never accepts a task, and a run with no required checks never accepts one. It is on
by default; set it to `false` to keep the `workgraph task accept` step (see
[Task lifecycle](task-lifecycle.md#acceptance)).

### Issue trackers

Each `[trackers.<name>]` table connects Workgraph to one Jira site, read-only: Workgraph only issues
GET requests and never comments on, transitions or edits a Jira issue. Issues from the listed
`projects` are mirrored as tasks into the workspace named by `workspace`. The maintenance leader
queues an incremental sync every `poll_interval_seconds` and a full sync every
`full_sync_interval_seconds`, and `workgraph tracker sync` runs one on demand. The design, including
the status mapping, is in [design/tracker-sync.md](design/tracker-sync.md).

**Credentials.** The configuration names where the token comes from, never the token itself. Set
exactly one of:

- `token_env`: an environment variable holding an API token (Cloud) or a personal access token
  (Data Center).
- `token_command`: an argument vector (no shell) that prints the token on stdout, such as an OAuth
  helper. It runs with the user's environment, its output is held in memory only, and it is re-run
  when Jira answers 401, at most once a minute. It must finish within
  `token_command_timeout_seconds`.

On Cloud, `auth = "basic"` sends the token with the account email from `email_env`. With
`auth = "bearer"` the token is an OAuth 2.0 access token, `email_env` is not used, and `base_url` is
the API gateway, `https://api.atlassian.com/ex/jira/<cloud id>`. Set `site_url` to the site's own
address so that browse links point there. Data Center always sends a Bearer personal access token.

**Scope and rate limits.** `jql` narrows a large project: it is ANDed onto the incremental search and
onto the full sync's listing of keys. Parents and blockers outside the filter are still fetched by
key, and a full sync does not mark them missing while they exist in the listed projects.
`min_request_interval_seconds` spaces out requests. A response marked `X-RateLimit-NearLimit: true`
pauses the next request for at least 5 seconds. A 429's `Retry-After` is honoured in full up to five
minutes; a longer one fails the request rather than retrying early.

**Pull requests and assignees.** `pr_fields` are read for full pull request URLs (the scheme is
optional) and for `pull/42` shorthand or numbers listed after a URL (`pull/112, 114 and 120`), which
take the last URL's repository. `pr_default_repos` gives a field's repository for shorthand with no
URL before it, and for a field that holds only numbers (`1749, #1809`); without one, such references
are skipped. `assignee_field` names a user field to read instead of Jira's `assignee`, for sites where
the assignee is not the person building the work; a multi-user field gives its first user.

**Worktree root.** `[execution] worktree_root` (global or per workspace) puts attempt worktrees at
`<root>/<task-key>-<n>` instead of `<data>/worktrees/<attempt id>`. Short paths keep deep
repositories under the Windows path limit. The root may also hold your own worktrees: Workgraph only
removes worktrees it recorded, in attempt directories directly inside the root.

## Secrets

Credentials are read from environment variables named in configuration (`api_key_env`) and are
never written to configuration files or the database. The local API token is a random value in
`config/api-token` (created by `workgraph init`, readable only by the user where the OS supports it).

## Configuration migration

`schema_version` identifies the document version. Older documents are upgraded in memory on
load; `workgraph config migrate` rewrites them in the current version.
