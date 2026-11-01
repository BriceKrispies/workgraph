# Getting started

## Installation

| Requirement | Why |
| --- | --- |
| Rust toolchain (version pinned in `rust-toolchain.toml`) | Builds the `workgraph` binary |
| Git 2.40+ | Authoritative Git operations (ingestion, worktrees, diffs) |
| Bun | Builds the web UI, which the server embeds |
| Ollama (optional) | Local model classification, embeddings and narratives |
| Claude Code (optional) | Claude Code agent adapter and lifecycle hooks |

From a checkout, build the UI first so it is embedded, then the binaries:

```sh
bun install --cwd web
bun run --cwd web build          # writes web/dist
cargo build --release -p wg-cli
```

Copy `targetelease\workgraph.exe` and `targetelease\workgraph-demo-agent.exe` to
`%USERPROFILE%\.localin`. The MCP registration and the Claude Code hooks store that path, so
replacing the files in place keeps them working. Then create the workspace:

```sh
workgraph init
```

## Verification

```sh
workgraph doctor
workgraph claude status
claude mcp list
```

`workgraph doctor` checks Git, the schema, database integrity, configuration, inference providers,
agent adapters, workers, the hook spool and the UI build. `workgraph claude status` shows hook
installation and ingestion counts. `claude mcp list` should show the `workgraph` server connected.

## Rollback

The database schema and row formats are the same in the Python and Rust versions, so no data step
is needed. From a commit that still has the Python code (for example `main`):

```sh
uv tool install .
```

This restores the Python `workgraph` command at the same path.

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
workgraph init --workspace acme --prefix AC
workgraph repo register C:/code/auth-service
workgraph repo register C:/code/web-client
workgraph project create login --repo auth-service --repo web-client
workgraph task create "Refresh expired sessions" --repo auth-service --repo web-client --project login --ready
workgraph task create "Use refresh flow in UI" --repo web-client --depends-on AC-1 --ready
workgraph task ready
workgraph git sync
workgraph serve
```

Then open <http://127.0.0.1:8765/>.

`serve` also listens on this machine's local-network address, so a phone on the same Wi-Fi can
use the UI. It prints that address and a pairing code; open the address on the phone and enter
the code once. `workgraph serve --local` (or `server.lan = false`) keeps it on loopback only.

Agents typically:

```sh
workgraph --json task ready
workgraph task claim AC-1
workgraph task progress AC-1 "implemented the endpoint" --percent 60
workgraph git link AC-1 3f2c1ab
workgraph task complete AC-1 -m "endpoint and tests added"
```

and a human (or configured verification) accepts:

```sh
workgraph verify run AC-1
workgraph task accept AC-1
```

## Running agents (opt-in)

Agent code execution is disabled until you opt in:

```sh
workgraph config set execution.enabled true
workgraph agent list
workgraph run start AC-1 --agent claude_code --wait
```

`--wait` needs a worker: either `workgraph serve` (which starts one) or `workgraph worker run`.

## Demonstrations

```sh
workgraph demo e2e
workgraph --home ./demo-home demo seed
workgraph --home ./demo-home serve
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
workgraph config set llm.classification '{"provider": "heuristic", "model": "heuristic-v1"}'
workgraph config set llm.embeddings '{"provider": "heuristic", "model": "hash-embed-256"}'
workgraph config set llm.summarization '{"provider": "heuristic", "model": "heuristic-v1"}'
```

- The scripted command agent (`workgraph-demo-agent`) runs real worktree edits and
  commits without any external agent.
- The web UI is served from the local build; it loads no remote assets.
