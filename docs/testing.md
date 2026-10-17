# Testing

```sh
uv run pytest
uv run pytest -m "not slow and not e2e"
uv run pytest tests/e2e
uv run pytest --cov=workgraph --cov-report=term
uv run tools/check.py
```

The ordinary suite needs **no network, no Ollama, no Claude Code and no paid API**:

| Real thing | How tests exercise it |
| --- | --- |
| Git | real temporary repositories created per test (`tests/conftest.py::make_repo`) |
| Coding agents | `workgraph/demo/scripted_agent.py`, a real `workgraph.agent.v1` executable that edits worktrees and commits |
| Claude Code | `tests/fixtures/fake_claude.py` emitting documented `stream-json` messages behind a `claude` wrapper; hook payloads from the documented formats; the real `workgraph claude hook` entrypoint in a subprocess |
| Ollama | `tests/fixtures/fake_ollama.py`, a real HTTP server implementing `/api/tags`, `/api/chat`, `/api/embed` (with forced failures) |
| OpenAI-compatible servers | an in-test HTTP server |
| MCP | the real server over stdio (subprocess) and Streamable HTTP (uvicorn), driven by the SDK client |
| HTTP API | Starlette `TestClient` with the real security middleware, and uvicorn for MCP/e2e |
| Database | real SQLite files in WAL mode, real Alembic migrations |

Layout: `tests/unit` (pure logic, queue semantics, parsers, configuration, migrations),
`tests/integration` (services against real Git/SQLite/processes, API, MCP, CLI),
`tests/e2e` (the 25-step demonstration and demo seeding). Markers: `slow`, `e2e`.

## Frontend

```sh
pnpm --dir web typecheck
pnpm --dir web test
pnpm --dir web build
pnpm --dir web test:e2e
```

Component tests use Vitest and Testing Library. The Playwright suite starts the real backend
against an isolated, seeded home and drives the system Chrome (no browser download).

## Generating a migration

Alembic runs programmatically (`workgraph.persistence.migrate`). To autogenerate a revision,
upgrade a scratch database to the current head and call Alembic with that connection:

```sh
uv run python -c "from pathlib import Path; from alembic import command; from alembic.config import Config; from workgraph.persistence.db import Database; from workgraph.persistence import migrate; db = Database(Path('scratch.db')); migrate.upgrade(db); cfg = Config(); cfg.set_main_option('script_location', str(migrate.MIGRATIONS_DIR)); conn = db.engine.connect(); cfg.attributes['connection'] = conn; command.revision(cfg, message='describe change', autogenerate=True, rev_id='0003'); conn.commit()"
```

Review the generated file by hand (add `server_default` for new NOT NULL columns, keep FTS changes
in raw SQL), update `SCHEMA_HEAD` in `persistence/schema_version.py`, and add a test that upgrades a
populated database from the previous revision.
