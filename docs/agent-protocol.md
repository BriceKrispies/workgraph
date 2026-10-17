# Agent adapter protocol (`workgraph.agent.v1`)

The generic command adapter lets any executable act as a Workgraph agent without changes to the
domain model.

## Configuration

```toml
[agents.my-agent]
type = "command"
executable = "C:/tools/my-agent.exe"
args = ["run", "--manifest", "{manifest}", "--prompt", "{prompt_file}", "--attempt", "{attempt_id}"]
env = { MY_AGENT_MODE = "careful" }
timeout_seconds = 1800
supports_progress = true
supports_usage = true
supports_resume = false
```

or `uv run workgraph agent add-command my-agent C:/tools/my-agent.exe --arg run --arg "{manifest}"`.

- No shell is involved; every template element becomes exactly one argument.
- Allowed placeholders: `{attempt_id}`, `{task_id}`, `{task_key}`, `{workdir}`, `{manifest}`,
  `{prompt_file}`. Anything else is rejected when the configuration is loaded.
- The child gets a minimal environment (PATH, temp/home dirs, locale) plus `env` and the
  `WORKGRAPH_*` correlation variables. Secrets in the parent environment are not inherited.
- The working directory is the first writable repository's managed worktree.

## Input

On stdin (and in `workgraph-request.json` beside the manifest, outside every repository):

```json
{
  "protocol": "workgraph.agent.v1",
  "attempt_id": "…",
  "task": {"id": "…", "key": "WG-12"},
  "prompt": "# Task WG-12: …",
  "manifest": {
    "task_id": "…", "task_key": "WG-12", "attempt_id": "…", "workspace_dir": "…",
    "repositories": [
      {"repository_id": "…", "repository": "api", "checkout_path": "…/worktrees/<attempt>/api",
       "sha": "<baseline sha>", "branch": "wg/wg-12/attempt-1-1a2b3c4d", "access": "read_write",
       "isolation": "worktree", "target_ref": "main"}
    ],
    "expected_changes": ["api"],
    "verification": [{"name": "unit tests", "command": ["pytest"], "required": true}],
    "agent_checks": [{"name": "lint", "command": ["ruff", "check", "."], "repository": "api"}],
    "baseline_manifest_id": "…", "change_set_id": "…"
  }
}
```

The manifest is also written to `{manifest}` (`workgraph-manifest.json`). `agent_checks` lists the
fast-tier checks (`name`, `command`, `repository`) that an agent should run itself; `verification`
lists every check that Workgraph runs afterwards.

## Output

One JSON object per stdout line:

| `type` | Fields | Meaning |
| --- | --- | --- |
| `progress` | `message`, `percent?` | streamed to the attempt timeline and UI |
| `log` | `message` | free-form log line |
| `usage` | `input_tokens`, `output_tokens`, … (non-negative integers) | **measured** usage only; summed |
| `artifact` | `name`, `path`, `kind?` | an output file worth recording |
| `result` | `status` (`succeeded`/`failed`), `summary`, `error?`, `cost_usd?` | final outcome |

Non-JSON lines are kept as log output. Exit code 0 without a `result` counts as success with an
empty summary; any other exit code is a failure. Timeouts and cancellation kill the process tree.

## What happens afterwards

Workgraph - not the agent - captures the outcome manifest, attributes commits between the baseline
and outcome revisions, enforces read-only repositories, runs the approved verification checks
and moves the task to awaiting review (or failed). Commit with a `Task-ID: <KEY>` trailer, never
push or merge, and do not modify read-only repositories.

A reference implementation is `src/workgraph/demo/scripted_agent.py`.

## Writing a new built-in adapter

Implement the `AgentProvider` protocol in `integrations/agents/base.py` (`capabilities`,
`check_available`, `config_snapshot`, `run`) and register it in `integrations/agents/registry.py`.
Declare honestly whether the provider streams progress, reports usage or cost, supports resuming
sessions, runs in an isolated workspace or supports subagents/hooks.
