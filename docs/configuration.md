# Configuration

Configuration is TOML, validated by Pydantic schemas in `src/workgraph/config/settings.py`.
Unknown keys are rejected.

- `config.toml` - global settings.
- `workspaces/<slug>.toml` - per-workspace overrides (`llm`, `agents`, `execution`, `jobs`),
  deep-merged over the global file.

```sh
uv run workgraph config path
uv run workgraph config show
uv run workgraph config validate
uv run workgraph config migrate
uv run workgraph config set execution.max_concurrent_runs 3
uv run workgraph config set llm.classification.model qwen2.5:14b-instruct
uv run workgraph config set execution.enabled true --workspace acme
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

[jobs]
worker_concurrency = 4
gpu_concurrency = 1            # concurrent model jobs (resource key "gpu")
per_agent_concurrency = 2      # resource key "agent:<name>"
per_repository_concurrency = 1 # resource key "repo:<id>" (git jobs)
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

## Secrets

Credentials are read from environment variables named in configuration (`api_key_env`) and are
never written to configuration files or the database. The local API token is a random value in
`config/api-token` (created by `workgraph init`, readable only by the user where the OS supports it).

## Configuration migration

`schema_version` identifies the document version. Older documents are upgraded in memory on
load; `workgraph config migrate` rewrites them in the current version.
