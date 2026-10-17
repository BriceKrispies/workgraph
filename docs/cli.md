# CLI

`workgraph --help` lists every group; each command has `--help`. Global options:

| Option | Effect |
| --- | --- |
| `--json` (anywhere) | Machine-readable JSON on stdout; errors as `{"error": {"code","message","details"}}` |
| `--home DIR` | Use an isolated Workgraph home (same as `WORKGRAPH_HOME`) |
| `-w/--workspace SLUG` | Default workspace for the invocation (same as `WORKGRAPH_WORKSPACE`) |

Exit codes: `0` success, `1` unexpected error, `2` validation, `3` conflict (state, version,
busy resource), `4` not found, `5` permission denied, `6` provider unavailable, `7` configuration.

## Command groups

| Group | Commands |
| --- | --- |
| `init`, `doctor` | create storage/schema/first workspace; health checks (`--verify-events`) |
| `serve` | HTTP API + UI + Streamable HTTP MCP at `/mcp/` + in-process worker (`--no-worker`, `--no-mcp`) |
| `mcp` | MCP server over stdio (default) or HTTP; `--agent-name`, `--grant run:start` |
| `workspace` | `create`, `list`, `show`, `use`, `archive`, `set --allowed-root` |
| `project` | `create`, `list`, `show`, `update`, `add-repo`, `remove-repo` |
| `repo` | `register`, `list`, `show`, `refresh`, `status`, `update`, `remove`, `remove-checkout`, `depend` |
| `task` | `create`, `list`, `ready`, `show`, `update`, `bulk-update`, `claim`, `release`, `complete`, `accept`, `reject`, `comment`, `progress`, `result`, `depend`, `relate`, `link-repo`, `archive`, `history`, `graph` |
| `stack` | `create PARENT`, `show PARENT`, `list [--all]`, `integrate PARENT` (queues merging the ready children onto each repository's integration branch; verification and acceptance come in later stages) |
| `attempt` | `list`, `show`, `create`, `finish`, `compare`, `manifests` |
| `agent` | `list`, `add-command` |
| `run` | `start [--wait]`, `get`, `watch`, `cancel`, `retry`, `rerun`, `schedule [--dry-run/--explain]`, `decompose [--create]` |
| `verify` | `config`, `configs`, `run [--tier fast\|full]`, `runs`, `failures` |
| `worker` | `run [--once/--kind]`, `list`, `recover` |
| `job` | `list`, `show`, `stats`, `cancel`, `retry`, `enqueue`, `compact` |
| `git` | `sync [--full]`, `log`, `show`, `link`, `unlink`, `changes`, `diff`, `surface`, `analyze`, `worktrees`, `cleanup-worktrees [--apply]`, `install-hook` |
| `classify` | `request [--now]`, `show`, `confirm`, `promote`, `reclassify`, `gold`, `evaluate`, `evaluations`, `disagreements` |
| `search` | full-text / semantic / hybrid; `--similar-to KEY`; `--reindex` |
| `analytics` | `metrics`, `query`, `overview`, `compare`, `coupling`, `failures`, `estimate`, `history`, `export`, `retrospective` |
| `automation` | `list`, `create`, `update`, `show`, `enable`, `disable`, `trigger`, `preview`, `runs`, `notifications`, `catalog`, `process` |
| `claude` | `install [--mcp]`, `uninstall`, `status`, `replay`, `bind`, `unbind`, `sessions`, `mcp-config`, `hook` |
| `config` | `show`, `path`, `validate`, `migrate`, `set` |
| `backup`, `restore` | consistent online backup zip; verified restore with safety copy and forward migration |
| `export`, `import` | `export workspace FILE.jsonl`, `export analytics`; `import FILE.jsonl` |
| `events` | `list`, `verify` |
| `demo` | `e2e`, `seed` |

Worktree discovery: `repo register` and `repo refresh` run `git worktree list` and record each linked worktree (kind `worktree`) and the main working tree (kind `clone`) as a checkout of the same repository. Workgraph's managed worktrees, bare entries and paths outside the workspace's allowed roots are not added; the skipped paths are listed in the repository's `health_detail.skipped_worktrees`. Refresh marks worktrees that were removed or are reported as prunable as unavailable, and records `repository.checkout_discovered` / `repository.checkout_lost` events.

## Examples

```sh
uv run workgraph task create "Fix crash" --description @notes.md -c "no crash on expiry" -P high --ready -r api -t bug
uv run workgraph task list --status ready --priority high --repo api --search "token" --sort priority --asc
uv run workgraph --json task show WG-12
uv run workgraph task update WG-12 --status blocked --reason "waiting for infra" --expected-version 4
uv run workgraph task depend WG-13 WG-12
uv run workgraph task bulk-update WG-1 WG-2 WG-3 --add-tag sprint-42
uv run workgraph git sync --full
uv run workgraph git changes WG-12
uv run workgraph analytics query execution_duration --by provider --by model --since 2026-09-01
uv run workgraph analytics export facts.parquet --format parquet --dataset task_facts
uv run workgraph automation preview --rule "tag bugs" --task WG-12
uv run workgraph classify evaluate --provider ollama --model qwen2.5:7b-instruct
uv run workgraph verify run WG-12 --tier fast
```

All commands are single-line and work unchanged in PowerShell, bash, zsh and Nushell.
