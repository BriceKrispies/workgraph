# Workgraph

Workgraph is a **local-first, agent-first engineering control plane**. It tracks tasks across many
Git repositories, lets coding agents discover, claim and execute work through MCP, the CLI or an
HTTP API, ingests Git history to attribute real code changes to tasks, verifies results with
approved checks, and turns all of it into engineering analytics - entirely on your machine.

- **Tasks and dependency graphs** spanning projects and repositories, with an immutable event log.
- **Multi-repository workspaces**: canonical repository identities, multiple checkouts, managed
  Git worktrees per attempt, cross-repository change sets and revision manifests.
- **Agent-agnostic execution**: a Claude Code adapter (programmatic `claude -p --output-format
  stream-json`) and a generic command adapter speaking a small JSON-lines protocol.
- **MCP server** (stdio and Streamable HTTP) with 52 tools and resources over the same services.
- **Claude Code lifecycle hooks**: passive, idempotent, spooled ingestion of session, tool,
  subagent and internal-task events.
- **Git intelligence**: incremental ingestion, trailers, rename/merge/rebase/cherry-pick handling,
  attribution with provenance and confidence, Tree-sitter structural analysis for Python, C#,
  TypeScript, JavaScript, Go and Rust.
- **Local inference** (Ollama, OpenAI-compatible servers, or the built-in deterministic heuristic
  provider) for classification, embeddings, decomposition and narratives - always optional.
- **Durable SQLite job queue**, automation rules, dependency-aware scheduler, verification and
  acceptance.
- **DuckDB analytics**: throughput, execution, code churn vs final change, agents/models, quality,
  coupling, historical estimation, failure patterns and retrospectives.
- **Web UI** (React, TanStack Query, React Flow, Recharts, Tailwind) served by the local server.

Everything essential works offline, without accounts, hosted databases or paid APIs.

## Install

Requirements: Python 3.12+, [uv](https://docs.astral.sh/uv/), Git. For the web UI: Node.js 20+ and pnpm.

```sh
uv sync
uv run workgraph init
pnpm --dir web install
pnpm --dir web build
```

`uv tool install .` installs the `workgraph` command globally.

## Quick start

```sh
uv run workgraph repo register C:/code/my-service
uv run workgraph project create platform --repo my-service
uv run workgraph task create "Fix crash when the token expires" --ready --repo my-service --project platform
uv run workgraph task ready
uv run workgraph git sync
uv run workgraph serve
```

Open <http://127.0.0.1:8765/>. The server runs a background worker by default.

See it all working with no setup at all (three temporary repositories, MCP, agents, verification,
analytics, HTTP API and a restart):

```sh
uv run workgraph demo e2e
```

Explore the UI with realistic data in an isolated home:

```sh
uv run workgraph --home ./demo-home demo seed
uv run workgraph --home ./demo-home serve
```

## Connect an agent

```sh
uv run workgraph claude install --mcp
```

registers the MCP server with Claude Code and installs passive lifecycle hooks. Any MCP client can
run `workgraph mcp` over stdio. See [docs/mcp.md](docs/mcp.md).

## Quality checks

```sh
uv run ruff check .
uv run ruff format --check .
uv run ty check
uv run pytest
uv run tools/check.py
```

## Documentation

Work status, open limitations and follow-ups are tracked in Workgraph itself (repository
`workgraph`: `workgraph task list`), not in documents.

| Topic | Document |
| --- | --- |
| Overview and architecture | [docs/architecture.md](docs/architecture.md) |
| Data model | [docs/data-model.md](docs/data-model.md) |
| Installation, quick start, offline use | [docs/getting-started.md](docs/getting-started.md) |
| Configuration | [docs/configuration.md](docs/configuration.md) |
| CLI | [docs/cli.md](docs/cli.md) |
| MCP | [docs/mcp.md](docs/mcp.md) |
| Claude Code integration | [docs/claude-code.md](docs/claude-code.md) |
| Agent adapter protocol | [docs/agent-protocol.md](docs/agent-protocol.md) |
| Ollama and inference providers | [docs/inference.md](docs/inference.md) |
| Git integration and multi-repository workflows | [docs/git.md](docs/git.md) |
| Task lifecycle, verification, acceptance | [docs/task-lifecycle.md](docs/task-lifecycle.md) |
| Automation rules | [docs/automation.md](docs/automation.md) |
| Analytics definitions | [docs/analytics.md](docs/analytics.md) |
| Security boundaries | [docs/security.md](docs/security.md) |
| Backup and recovery | [docs/backup.md](docs/backup.md) |
| Extending Workgraph | [docs/extending.md](docs/extending.md) |
| Testing | [docs/testing.md](docs/testing.md) |
| Troubleshooting | [docs/troubleshooting.md](docs/troubleshooting.md) |
| HTTP API | [docs/api.md](docs/api.md) |

## Observed measurements versus model interpretations

Workgraph keeps a hard line between what it **measured** (Git statistics, timings it recorded,
check exit codes, provider-reported token usage) and what a model **inferred** (classifications,
decomposition proposals, retrospective narratives). Inferred values always carry provider, model,
prompt version and confidence, and are labelled as interpretations in every interface. Missing
telemetry is reported as missing - never estimated.
