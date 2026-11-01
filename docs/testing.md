# Testing

## Rules

- **Hard 2-minute ceiling.** No test run takes longer than 120 s of wall-clock time. Budgets enforce
  it: `wg-conformance --budget SECS` (default 120, refused above 120) kills unfinished scenarios and
  fails the run, and `tools/check.py` runs each test step under the same limit and kills its process
  tree when it is exceeded. A check that does not fit is cut or split, never given more time.
- **Conformance `check` compares behaviour.** For each step it compares the exit code and HTTP
  status. When both stdouts are JSON it compares their shape: the same object keys recursively, the
  same value kind per value, the same array lengths, ignoring scalar values and the normaliser's
  placeholders. When stdout is not JSON it compares only whether both are empty or both non-empty.
  Explicit `assert` steps in a scenario apply exactly. Stderr is not compared.
- **`diff` mode is for investigation only.** It runs a scenario against a reference and a candidate
  and compares bytes. It needs a reference implementation (`--python COMMAND`, an older Workgraph
  build) and is never a gate.
- **Tests run on stacked changes.** A single change runs only `cargo check`, `cargo clippy -- -D
  warnings` and `cargo fmt`. The test suites run once on the combined stack.

## Commands

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo purity                      # purity tiers and the shell logic ceiling
cargo lint-arch                   # architecture lints
cargo test --workspace --no-run   # build the test binaries (unbudgeted)
cargo test -p <crate>             # one crate's tests
python tools/check.py --tests     # the gate: fmt, clippy, purity, cargo test, conformance check, web
```

`tools/check.py` uses only the Python standard library to drive these steps. `--no-web` skips the
frontend steps.

## Conformance

`crates/wg-conformance` builds `wg-conformance`. Its inputs are in `tests/conformance`:

| Path | Contents |
| --- | --- |
| `scenarios/` | Language-neutral scenario files (CLI, HTTP and MCP steps), validated by `scenario.schema.json` |
| `golden/` | Recorded outputs that `check` compares against |
| `divergences.toml` | Reviewed, allowed differences |
| `coverage-allowlist.toml` | Contract ids excluded from coverage, with reasons |
| `pyclock/` | A clock shim used to re-record goldens from an older Workgraph build |

Modes: `validate`, `check`, `diff`, `record`, `coverage`, plus `dbdump`, `mcp-replay` and
`replay-http`.

```sh
wg-conformance check --rust "target/debug/workgraph" --jobs 8 \
  --scenarios tests/conformance/scenarios --golden tests/conformance/golden
```

`--python` is optional and only used by `diff` and `record`. `tools/conformance_budget.py` picks
which scenarios to keep under the budget from a `check` report.

Other fixtures: `golden/` holds vectors recorded for the pure modules, read by crate tests.
`contracts/` holds snapshots of the CLI tree, the OpenAPI document and the MCP descriptors, plus
recorded HTTP and MCP transcripts. Contract-coverage tests check the groups they list as
implemented. `tests/fixtures/jira` holds Jira payloads for the tracker tests.

## Frontend

```sh
bun run --cwd web typecheck
bun run --cwd web test       # Vitest and Testing Library
bun run --cwd web build
```

Browser tests (`web/e2e`, Playwright) start the `workgraph` binary on the `PATH`, or the one named
by `WORKGRAPH_BIN`, in a throwaway home seeded with `demo seed`.
