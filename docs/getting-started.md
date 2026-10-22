# Getting started

## Installation

| Requirement | Why |
| --- | --- |
| Python 3.12+ and [uv](https://docs.astral.sh/uv/) | Runs Workgraph and manages its environment |
| Git 2.40+ | Authoritative Git operations (ingestion, worktrees, diffs) |
| Node.js 20+ and pnpm (optional) | Builds the web UI |
| Ollama (optional) | Local model classification, embeddings and narratives |
| Claude Code (optional) | Claude Code agent adapter and lifecycle hooks |

From a checkout:

```sh
uv sync
uv run workgraph init
pnpm --dir web install
pnpm --dir web build
```

`uv run workgraph doctor` checks Python, Git, the schema, database integrity, configuration,
inference providers, agent adapters, workers, the hook spool and the UI build.

To install the command for your user: `uv tool install .` (then use `workgraph` directly).

## Where data lives

| What | Default (platformdirs) | With `--home DIR` / `WORKGRAPH_HOME` |
| --- | --- | --- |
| Configuration (`config.toml`, `workspaces/*.toml`, `api-token`) | user config dir | `DIR/config` |
| Database, artifacts, managed worktrees, spool, backups | user data dir | `DIR/data` |
| Logs | user log dir | `DIR/logs` |

`workgraph config path` prints the actual locations. The database is never placed inside a Git
repository.

## Quick start

```sh
uv run workgraph init --workspace acme --prefix AC
uv run workgraph repo register C:/code/auth-service
uv run workgraph repo register C:/code/web-client
uv run workgraph project create login --repo auth-service --repo web-client
uv run workgraph task create "Refresh expired sessions" --repo auth-service --repo web-client --project login --ready
uv run workgraph task create "Use refresh flow in UI" --repo web-client --depends-on AC-1 --ready
uv run workgraph task ready
uv run workgraph git sync
uv run workgraph serve
```

Then open <http://127.0.0.1:8765/>.

`serve` also listens on this machine's local-network address, so a phone on the same Wi-Fi can
use the UI. It prints that address and a pairing code; open the address on the phone and enter
the code once. `workgraph serve --local` (or `server.lan = false`) keeps it on loopback only.

Agents typically:

```sh
uv run workgraph --json task ready
uv run workgraph task claim AC-1
uv run workgraph task progress AC-1 "implemented the endpoint" --percent 60
uv run workgraph git link AC-1 3f2c1ab
uv run workgraph task complete AC-1 -m "endpoint and tests added"
```

and a human (or configured verification) accepts:

```sh
uv run workgraph verify run AC-1
uv run workgraph task accept AC-1
```

## Running agents (opt-in)

Agent code execution is disabled until you opt in:

```sh
uv run workgraph config set execution.enabled true
uv run workgraph agent list
uv run workgraph run start AC-1 --agent claude_code --wait
```

`--wait` needs a worker: either `workgraph serve` (which starts one) or `workgraph worker run`.

## Demonstrations

```sh
uv run workgraph demo e2e
uv run workgraph --home ./demo-home demo seed
uv run workgraph --home ./demo-home serve
```

`demo e2e` runs the 25-step cross-repository scenario in a temporary home and deletes it
afterwards (`--keep` retains it). `demo seed` fills an isolated home with real repositories,
commits, runs, verification results and analytics for exploring the UI. Neither ever runs
implicitly.

## Working completely offline

Nothing in Workgraph requires network access:

- SQLite and DuckDB are embedded; analytics use no DuckDB extensions.
- Git operations are local.
- Inference is optional. With no Ollama running, model jobs fail with a clear
  `provider_unavailable` error after their retries while everything else keeps working.
  For fully offline enrichment select the built-in heuristic provider:

```sh
uv run workgraph config set llm.classification '{"provider": "heuristic", "model": "heuristic-v1"}'
uv run workgraph config set llm.embeddings '{"provider": "heuristic", "model": "hash-embed-256"}'
uv run workgraph config set llm.summarization '{"provider": "heuristic", "model": "heuristic-v1"}'
```

- The scripted command agent (`workgraph/demo/scripted_agent.py`) runs real worktree edits and
  commits without any external agent.
- The web UI is served from the local build; it loads no remote assets.
