# Testing

Every test run has a hard **120 s wall-clock budget**. It is enforced by `wg-conformance --budget`
(default 120, refused above 120) and by `tools/check.py`, which runs each test step under the same
120 s limit. A run that outlives the budget is killed and fails. A check that does not fit the
budget is cut, never lengthened.

```sh
cargo test --workspace --no-run   # build the test binaries (unbudgeted)
cargo test -p <crate>             # run one crate's tests; must finish in 120 s
uv run tools/check.py --tests     # stack verification: the Rust tests run under the budget
wg-conformance check --rust COMMAND --budget 60   # a conformance run with a shorter budget
```

## Definition of done

The port is done when three things hold. It is not measured by byte equality with Python.

1. Rust's own tests pass (`cargo test --workspace`).
2. `wg-conformance check` passes. It runs each scenario against the Rust binary and compares
   behaviour with the recorded golden: each step's exit code and HTTP status; when both stdouts are
   JSON, their shape (the same object keys recursively, the same value kind per value, the same
   array lengths, ignoring scalar values and the normaliser's placeholders); when stdout is not
   JSON, only whether both are empty or both non-empty. Explicit `assert` steps in scenarios apply
   exactly. Stderr is not compared.
3. The data compatibility check passes (`tools/compat_check.py`).

`wg-conformance diff` keeps the exact byte comparison against a live Python reference. It is for
investigation, not a gate.

Tests run once per stack, not per change. A per-change agent runs only the cheap checks
(`cargo check`, `cargo clippy -D warnings`, `cargo fmt`).

## Frontend

```sh
bun run --cwd web typecheck
bun run --cwd web test
bun run --cwd web build
```

Component tests use Vitest and Testing Library. `tools/check.py` runs `web test` under the budget.

## Generating a migration

Alembic runs programmatically (`workgraph.persistence.migrate`). To autogenerate a revision,
upgrade a scratch database to the current head and call Alembic with that connection:

```sh
uv run python -c "from pathlib import Path; from alembic import command; from alembic.config import Config; from workgraph.persistence.db import Database; from workgraph.persistence import migrate; db = Database(Path('scratch.db')); migrate.upgrade(db); cfg = Config(); cfg.set_main_option('script_location', str(migrate.MIGRATIONS_DIR)); conn = db.engine.connect(); cfg.attributes['connection'] = conn; command.revision(cfg, message='describe change', autogenerate=True, rev_id='0003'); conn.commit()"
```

Review the generated file by hand (add `server_default` for new NOT NULL columns, keep FTS changes
in raw SQL), update `SCHEMA_HEAD` in `persistence/schema_version.py`, and add a test that upgrades a
populated database from the previous revision.
