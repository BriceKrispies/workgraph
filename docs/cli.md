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
| `task` | `create`, `list`, `ready`, `show`, `update`, `bulk-update`, `claim`, `release`, `complete`, `accept`, `reject`, `comment`, `progress`, `result`, `depend`, `relate`, `link-repo`, `archive`, `history`, `graph` (`--project`, `--repo`, `--open`) |
| `experiment` | `create SLUG --title T [--question Q] --protocol @file.json\|JSON`, `list [--status]`, `show EXP` (protocol summary and trials in order), `freeze EXP` (validates the protocol, freezes it and starts the experiment), `close EXP`, `trial add EXP TASK --arm A --phase P [--band N]` (a planned trial of a running experiment), `trial show TRIAL`, `trial start TRIAL` (clones the task's replay base into a fresh single-branch clone with no remote, runs the protocol's `setup`, prints the worktree, branch, base sha and brief), `trial grade TRIAL [--timeout S]` (runs the oracle and gates in the clone and records the outcome; a failure with flags is classed `gaming`), `trial classify TRIAL --cause C --source human\|agent\|mechanical [--notes N]` (see [Experiments](data-model.md#experiments)) ; `replay scan --repo R [--rev REV] [--limit N] [--min-lines N] [--max-lines N] [--include-excluded]` (commits that could become replay tasks, with size band and exclusion reasons), `replay create --repo R SHA... [--validate] [--oracle-template JSON-list] [--env K=V]... [--timeout S]` (one replay task per commit: `created`, `exists`, `skipped` or `rejected`) |
| `stack` | `create PARENT`, `show PARENT` (repositories with base, integration branch and tip; children in merge order), `list [--all]`, `add PARENT TASK` and `remove PARENT TASK` (change the children of the active stack), `history PARENT` (the stack's events in order, with their verification runs), `integrate PARENT` (queues merging the ready children onto each repository's integration branch), `verify PARENT` (queues full verification of the integration tip, bisecting and ejecting a failing child). Acceptance goes through `task`: `task accept PARENT` (or `--manual --reason`) lands a verified stack on the default branch, refused with `landing: deferred` when the default branch moved and the stack is rebuilt to verify again; `task accept CHILD` accepts one child alone and removes it from its stack; `task reject PARENT` leaves the stack as it is (see [Git](git.md#stack-landing)) |
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
| `ui` | `screenshot [PAGE]`: capture a page of the running web UI to a PNG (see [below](#ui-screenshots)) |
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

## UI screenshots

`workgraph ui screenshot PAGE -o FILE.png` loads a page of the web UI served by `workgraph serve` in a
headless browser and writes a PNG. `PAGE` is the UI path with its query string, so any filtered view
can be captured:

```sh
workgraph ui screenshot "graph?repo=rho&linked=1" -o rho-graph.png --width 2400 --height 1500 --scale 2
workgraph ui screenshot tasks/WG-12 -o wg-12.png
```

No browser is bundled: Chrome, Edge or Chromium is found on `PATH` or in its standard install location,
or named with `--browser` / `WORKGRAPH_BROWSER`. The capture waits until nothing on the page is marked
`aria-busy` and, for pages that load their own data, until that data has rendered (`--wait-for SELECTOR`
overrides this), then `--settle` seconds. The server address comes from the config unless `--url` is given.

The graph page reads its filters from the URL: `project`, `repo`, `open=1` (hide accepted and cancelled
tasks), `linked=1` (only tasks with a dependency, parent or relation edge) and `relations=0`. Tasks with no
dependency or parent edge are laid out in a grid below the dependency layers.
