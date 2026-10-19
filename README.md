# Workgraph

Workgraph is a local-first task tracker and control plane for agentic engineering. Coding agents find,
claim and finish work through MCP, the CLI or an HTTP API. Workgraph reads Git history to link real
commits to tasks, checks results with approved verification commands, and reports analytics on
how the work went. Everything runs on your machine with SQLite. It needs no accounts and no hosted services.

- Tasks and dependency graphs that span many repositories
- An MCP server, a CLI, an HTTP API and a web UI, all built on the same services
- Git ingestion and commit attribution using `Task-ID:` trailers
- Verification, stacked integration and automatic acceptance
- A read-only Jira sync that treats Jira tickets as the source of truth

## Install

Requires Python 3.12+, [uv](https://docs.astral.sh/uv/) and Git. The web UI also needs Node.js 20+ and pnpm.

```sh
uv sync
uv run workgraph init
pnpm --dir web install && pnpm --dir web build
uv tool install .   # optional: install the `workgraph` command globally
```

## Use

```sh
workgraph repo register path/to/repo
workgraph task create "Fix token expiry crash" --ready --repo repo
workgraph task ready
workgraph serve        # http://127.0.0.1:8765/
workgraph demo e2e     # full walkthrough in temporary repositories
```

The [`docs/`](docs/) folder covers the CLI, the MCP tools, configuration and the data model.
