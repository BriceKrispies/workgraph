# Workgraph in Rust: design and task breakdown

> Status: design, for decision. Task: WG-3362. Measured at `5aecab8` (`main`, 2026-10-25).
>
> Direction from the user (2026-10-25): convert Workgraph to Rust "in the Rust way, properly", as a
> ground-up rewrite in purely functional Rust (a pure core of immutable values and pure state
> transitions that return effects as data, and a thin imperative shell for I/O), keeping every
> external contract identical, and planned as many small tasks that Haiku agents can do in parallel.

The contract inventory is in [rust-rewrite-contracts.md](rust-rewrite-contracts.md). Counts in this
document are measured unless labelled otherwise. There are no duration estimates anywhere in it;
work is ordered by dependency.

---

## 0. Summary

* **What must not change.** 192 CLI commands under 32 top-level names, 8 exit codes, the `--json`
  and human output; 69 MCP tools, 2 resources and 5 resource templates with their exact JSON
  Schemas; 130 HTTP operations over 114 paths, 81 component schemas and 7 routes outside the OpenAPI
  document (SPA, pairing, `/mcp/`, Swagger); a 73-table SQLite schema at Alembic revision `0011`, with
  existing databases opening unchanged; 38 configuration keys and 3 named-table families in TOML
  files; the environment variables Workgraph reads (E01-E13) and sets for children (E20-E24); the
  hook, agent, git and verification protocols. The inventory gives each of them an id.
* **Shape.** A Cargo workspace at the repository root with 16 `Core` crates (the domain: tasks,
  registry, git logic, execution, stacks, verification, jobs, automation, inference, telemetry,
  trackers, experiments, analytics, events), 6 `Pure` support crates (Python-compatible JSON,
  Python-compatible RNG, wire DTOs, request validation, rich-compatible rendering, configuration),
  9 impure shell crates (SQLite store, git, processes, agent adapters, HTTP clients, tree-sitter,
  archives, browser, and the runtime that interprets effects), 3 interface crates (`wg-cli` builds
  the `workgraph` binary, `wg-server`, `wg-mcp`), the companion binary `workgraph-demo-agent`, and
  two tooling crates.
* **Purity model.** Every command is `load slice -> decide -> apply`: the shell reads a slice of the
  database into persistent (`rpds`) values, a pure function returns a `Decision` (row writes, domain
  events, effects as data, and the reply), and the shell applies the writes and events in the same
  `BEGIN IMMEDIATE` transaction, then interprets the effects. Long flows (agent runs, verification,
  stack integration, bisection and landing, git sync, tracker sync) are pure state machines
  `step(state, input) -> (state, effects)` whose inputs include the results of the effects. Time,
  ids and randomness are stamped by the shell into each step's input. This differs from noether's
  pure-core-v2 in one deliberate way: there is no in-memory world folded from a log, because
  Workgraph's state is a relational database that must stay readable by the Python version; the
  slice is the world for one decision (section 4.1).
* **Purity gate.** noether's `arch-lint` purity checker is copied (not shared) and its `Core` rules
  are parameterised by a rules file (section 4.6).
* **Proof of parity.** Five layers: contract snapshots checked in and guarded by a drift test; golden
  vectors dumped from the Python functions for every pure module; a language-neutral scenario corpus
  run by a Rust differential runner against both binaries, comparing normalised output and a
  canonical database dump after every step; the existing Python interface tests run against the Rust
  binary through a harness switch; and database round trips in both directions plus migration from
  every revision. Parity means zero differences outside a reviewed allowlist (section 3.1).
* **Cut-over.** Both implementations share one database during the rewrite, so a schema change
  lands in both in one stack: the Alembic revision and its Rust port (D9; `0011`, structured task
  decisions, is the first). The Rust binary is installed beside the Python one as
  `workgraph-rs`, then replaces `~/.local/bin/workgraph.exe` at the same path, so the MCP
  registration and the Claude Code hooks keep working. Rollback is reinstalling the Python tool.
  Python is deleted once the corpus has zero unexplained differences and the user ends the soak.
* **Work.** 207 tasks over 16 dependency levels: 186 sized for Haiku, 17 marked `Tier: mid`, 4
  marked `Tier: top`. The widest level holds 43 tasks; because the 36 scenario-porting tasks are
  needed only at the parity stage, up to 76 tasks can be in flight at once (section 9.2).

---

## 1. Goals, non-goals and what "identical" means

Goals:

1. A Rust `workgraph` binary that a user or an agent cannot tell apart from the Python one, except
   by speed, across every contract in the inventory.
2. Idiomatic Rust: a Cargo workspace with crates along domain boundaries, typed errors, no
   `unwrap` in library code, no ORM-shaped or service-locator-shaped code, no `Any`-typed bags of
   attributes, and async only where a transport needs it.
3. A natively pure core: domain state as immutable values, transitions as pure functions, effects as
   data, enforced by a lint gate rather than by convention.
4. A plan made of small, independent, well-specified tasks.

Non-goals:

* Changing any contract, even to fix one. A defect found during the rewrite is reproduced in Rust and
  filed as a Workgraph task against both, unless the user decides otherwise (Q3).
* Rewriting the web UI. `web/` stays TypeScript; the Rust server serves its build output.
* New features during the rewrite.

"Identical" is defined precisely in section 3.1.

---

## 2. Contract inventory

[rust-rewrite-contracts.md](rust-rewrite-contracts.md) lists every contract with an id:

| Ids | Contract |
| --- | --- |
| C001-C192 | CLI leaf commands, arguments and options |
| M01-M69 | MCP tools, plus the resource table |
| H001-H130, X1-X7 | HTTP operations and routes outside OpenAPI |
| sections 13.4, 11 | database tables, indexes, triggers, migration chain |
| section 13.5 | configuration keys and typed sub-tables |
| E01-E24 | environment variables |
| F01-F15 | files and directories |
| O01-O15 | output formats |
| P01-P08 | hook, agent, git, verification, tracker and model protocols |
| section 10 | enums, task transitions, event types, job kinds, permissions, error codes, actors |
| B01-B12 | behaviour that spans calls |

Section 13 of that file is generated from snapshots of the running Python code: the Typer command
tree, the MCP server's own listings, the FastAPI OpenAPI document, `sqlite_master` and the
configuration JSON Schema. Task CF-01 checks those snapshots in under `contracts/` with a pytest
that fails when the Python code and the snapshot disagree. A Python change that alters a contract
therefore has to update the snapshot in the same commit, which makes the drift visible and is the
trigger for the matching Rust task (Q3).

---

## 3. Conformance strategy

### 3.1 Parity, defined

The Rust binary is at parity when, for every scenario in the conformance corpus and every test in
the Python interface suite, each of the following holds after normalisation (section 3.4), and every
remaining difference matches an entry in `tests/conformance/divergences.toml` that names a decision
in this document:

1. **CLI.** Equal exit code. Equal stdout bytes in `--json` mode. Equal stdout bytes in human mode
   with stdout not a terminal, `COLUMNS=120`, `NO_COLOR=1`. Equal stderr bytes for domain errors
   (`error (<code>): <message>` and detail lines). For Click usage errors, equal exit code 2 and equal
   error message text (layout is DV1).
2. **MCP.** Equal `initialize` result, equal `tools/list`, `resources/list` and
   `resources/templates/list` JSON. For every call: equal `isError`, equal structured content, equal
   text content.
3. **HTTP.** Equal status, equal `Content-Type`, equal security headers, equal JSON body; for SSE,
   equal event names and data for the same event sequence; for the SPA, equal bytes for every static
   file and for `index.html` with the token meta tag.
4. **Database.** After every step, an equal canonical dump: every table except `alembic_version`
   and the FTS shadow tables, rows sorted by primary key, JSON columns parsed and re-serialised,
   then normalised. `sqlite_master` equal for databases created or migrated by either binary.
5. **Files.** Equal configuration file bytes, equal spool record shape, equal names and contents of
   manifests and request files, equal worktree paths and branch names, equal settings.json edits.
6. **Git.** Equal refs, equal commit graph shape, equal trees and messages of commits Workgraph made
   (integration merges), equal identity on those commits.

Parity is reported per source (scenario file, Python test file) as equal / allowlisted / failed. The
cut-over task requires a failed count of zero on the integration branch.

### 3.2 Layers of evidence

| Layer | What it proves | Built by |
| --- | --- | --- |
| L1 Contract snapshots | the Rust interfaces declare exactly the Python commands, options, tools, schemas, routes, DDL and config keys | CF-01 to CF-06; each interface task compares its declarations with the snapshot in a test |
| L2 Golden vectors | each pure module computes what the Python function computes, input by input | GV-01 to GV-21 dump vectors from Python; the matching CORE/LIB task makes them pass |
| L3 Differential scenario corpus | whole command sequences behave the same, including database effects | CR-01 to CR-07 (runner), SC-01 to SC-36 (scenarios) |
| L4 Python interface tests in Rust mode | the 164 existing interface test functions pass against the Rust binary unchanged | TH-01 to TH-07, VF-01 |
| L5 Data compatibility | a database moves freely between implementations and migrates from every revision; Python and Rust workers share a queue | CF-05, VF-03, VF-04 |

**L2, golden vectors.** For every module that is pure in Python (or can be made so for the dump), a
deterministic script in `tools/golden/` imports the Python function, runs it over a curated and a
generated set of inputs, and writes `golden/<name>/*.json` (input, output). The Rust crate's tests
load the same files. The vectors stay after Python is retired; they become ordinary fixtures. The
502 in-process Python test functions are distributed: unit tests of pure functions become golden
vectors and Rust unit tests in the matching CORE task, and service-level integration tests become
scenarios (L3).

**L3, the differential runner.** `crates/wg-conformance` (tooling) runs scenario files against an
implementation given as an executable path plus environment, in a fresh `WORKGRAPH_HOME` and a fresh
temporary directory for repositories. It has three modes: `diff` (run Python and Rust, compare),
`record` (run Python, write goldens), and `check` (run Rust, compare with goldens). After Python is
retired only `check` remains. Scenario format (JSON, validated by `tests/conformance/scenario.schema.json`):

```json
{
  "id": "stack-bisect-ejects-culprit",
  "covers": ["C135", "C143", "C144", "M16", "B04"],
  "setup": [
    {"git": "init", "repo": "api"},
    {"git": "commit", "repo": "api", "files": {"check.txt": "ok"}, "message": "base"}
  ],
  "steps": [
    {"cli": ["init"]},
    {"cli": ["repo", "register", "{repo:api}"], "json": true},
    {"mcp": "task_create", "args": {"title": "Parent", "repositories": ["api"]}},
    {"http": "POST", "path": "/api/v1/ws/{ws}/stacks", "body": {"parent": "{key:1}"}},
    {"drain": {"max_jobs": 50}},
    {"assert": {"db": "stack_items", "where": {"state": "ejected"}, "count": 1}}
  ],
  "not_observable": []
}
```

Placeholders (`{repo:api}`, `{ws}`, `{key:1}`) are resolved from earlier outputs. `drain` runs
`workgraph worker run --once` until the queue is empty or the bound is reached, so background work
happens at defined points. `assert` steps hold the assertions the original Python test made, so a
scenario is meaningful on its own, not only as a differential. `not_observable` lists original test
cases whose behaviour cannot be reached through an interface, each with the golden-vector task that
covers it instead.

**L4, the harness switch.** `WORKGRAPH_IMPL=python|rust` and `WORKGRAPH_RUST_BIN=<path>` in the
Python test session (TH-01). Under `rust`: the CLI helper spawns the binary instead of calling Typer's
`CliRunner`; MCP stdio tests launch the binary; HTTP tests talk to `workgraph serve --no-worker` on a
free port instead of Starlette's `TestClient`. Setup that the tests do through Python services
(`app.tasks.create(...)`) keeps running in Python and writes the shared database, which the Rust
binary then reads, so every interface test is also a Python-writes, Rust-reads test.

**L5, data compatibility.**

* Fixture databases at every revision `0001` to `0011`, populated by Python (CF-05). The Rust
  binary migrates each to head; the canonical dump and `sqlite_master` must equal Python's result
  for the same file.
* Python writes, Rust reads: the full corpus run with Python, then every read command and resource
  replayed with Rust against the final database.
* Rust writes, Python reads: the corpus run with Rust, then `workgraph doctor --verify-events` and
  `events verify` with Python, and every read replayed with Python.
* Mixed queue: a Python worker and a Rust worker drain one queue concurrently (VF-04); job payloads,
  leases, heartbeats, idempotency keys, maintenance leadership and spool records are read and
  written by both.

### 3.3 MCP and HTTP transcript replay

CF-03 records raw JSON-RPC exchanges with the Python MCP server (stdio and Streamable HTTP):
`initialize`, the three listings, one successful and one failing call per tool, a schema
validation failure, and an unknown tool. CF-04 records the HTTP surfaces outside the OpenAPI
operations: the OpenAPI document, `/api/docs`, `/pair` in each state, 401, 404, 405, 421 and 422
bodies, the security headers, and an SSE stream sample. These are replayed against Rust by CR-04
and CR-05 in addition to the scenarios.

### 3.4 Normalisation

Two runs never produce the same UUIDs, timestamps or temporary paths. The normaliser (CR-02)
rewrites them consistently within one run, so structure and cross-references are still compared:

| Value | Placeholder | Rule |
| --- | --- | --- |
| UUIDs | `<id:N>` | numbered by first appearance across all outputs and the database dump of the run |
| Timestamps (any of the formats O01, O11) | `<ts:N>` | numbered by rank of the distinct values in the run, so ordering is compared; the format is kept (`<ts:3|Z>`, `<ts:3|+00:00>`, `<ts:3|db>`, `<ts:3|display>`) |
| Durations (`*_ms`, `duration_seconds`, wall-time fields) | `<dur>` | by field name allowlist |
| Git SHAs | `<sha:N>` | numbered by first appearance; commit graph compared separately |
| Temporary roots, home, ports, pids, worker ids, API token, pairing code | `<tmp>`, `<home>`, `<port>`, `<pid>`, `<worker:N>`, `<token>`, `<code>` | literal replacement |
| Task keys | kept | keys are allocated deterministically from the workspace counter |

Floating-point values are compared exactly, except fields listed in the allowlist as derived from
transcendental functions (`exp`, `log`, `log1p` in the experiment statistics), which are compared to
a relative tolerance of `1e-12` (DV7, Q9).

### 3.5 Coverage accounting

Every scenario declares `covers`. CR-07 reports, for each contract id in the inventory, the
scenarios and Python tests that exercise it, and lists ids nothing exercises. The coverage tasks
(SC-25 to SC-36) close those gaps; VF-02 requires every C, M, H and X id to be covered by at least
one scenario step, or by an L4 test, or to be listed with a reason (for example `ui screenshot`
needs a browser and is covered by its Python test only on machines that have one).

### 3.6 Deliberate divergences (initial allowlist, each needs the user's confirmation)

| Id | Divergence | Why |
| --- | --- | --- |
| DV1 | `--help` and Click usage-error layout. Commands, options, metavars, help strings and exit code 2 are identical; the boxes and wrapping that Typer draws with rich are not reproduced byte for byte | Typer's rich help panels are a large renderer for text nobody parses; content parity is checked against the snapshot (Q1) |
| DV2 | Human output piped on Windows is UTF-8. Python writes the console code page (cp1252) with `?` for unencodable characters | Python's behaviour loses characters; `--json` output is ASCII-escaped in both, so machine output is unaffected (Q2) |
| DV3 | `<data>/analytics.duckdb` is not created | analytics run in Rust without DuckDB (D12); the file is internal and was a cache |
| DV4 | `demo e2e` and `demo seed` configure the scripted agent and the demo checks as `workgraph-demo-agent ...` instead of `<python> scripted_agent.py ...` | a Rust installation has no Python; the step results are otherwise identical (Q8) |
| DV5 | Texts that only exist because the implementation is Python: exception class names in `spool_reason` and in the message of an unexpected error (exit 1), traceback text in logs, logger names | not contracts; the error envelope and codes are unchanged |
| DV6 | Parquet export bytes | compared by reading both files back (schema, row values) |
| DV7 | Last-ulp differences in statistics computed through `exp`/`log` | CPython and Rust may reach different libm implementations; tolerance `1e-12` relative, only for the listed fields (Q9) |
| DV8 | `doctor` check `python` reports `not required (native binary)` and always passes. Python runs the check `sys.version_info >= (3, 12)` and prints the interpreter version. Confirmed by the user 2026-10-26 | the Rust binary has no Python runtime to version-check; the check keeps its name and position in the list |
| DV9 | `doctor` has no `web frontend build` check. Python warns when `static/index.html` is missing. Confirmed by the user 2026-10-26 | the web UI is embedded in the binary at build time, so there is no static directory that can be missing |
| DV10 | Line endings: Rust writes `
` on every platform. Python writes `
` to redirected stdout on Windows (text-mode streams). The normaliser compares CRLF and LF as equal. Decided 2026-10-26 | no consumer parses the line terminator; JSON parsers accept both |

---

## 4. Architecture

### 4.1 The functional core and how it differs from noether's

noether's pure-core-v2 folds a log of records into an in-memory world value per tenant and steps it.
Workgraph cannot do that without changing contracts: its state of record is a relational SQLite
database that the Python version must keep reading and writing during the rewrite, with 73 tables,
an append-only event log that does not contain every field, and data (commits, telemetry) that is
far larger than any one command needs. So the unit of purity here is **one decision over one slice**.

```text
   request (CLI args, MCP call, HTTP request, job, maintenance tick, effect result)
        |
        v
   [shell]  parse + validate  ->  Command value            (wg-validate, wg-wire)
   [shell]  BEGIN IMMEDIATE (or DEFERRED for reads)
   [core]   reads(&Command) -> ReadSet                      (pure: which rows are needed)
   [shell]  load ReadSet -> Slice                           (wg-store; rpds values)
   [core]   decide(&Slice, &Command, &Stamp) -> Result<Decision, WgError>
   [shell]  apply Decision.writes + Decision.events, COMMIT
   [shell]  interpret Decision.effects (git, processes, HTTP, files, job enqueue wake-ups)
   [shell]  render Decision.reply                           (wg-wire + wg-json / wg-render)
```

```rust
/// Written by the shell for every decision; the only source of time and randomness.
pub struct Stamp { pub at: Timestamp, pub entropy: [u8; 32], pub actor: Actor, pub correlation: Uuid }

pub struct Decision<R> {
    pub writes: Vector<Write>,        // row-level inserts, updates (with expected version), deletes
    pub events: Vector<EventDraft>,   // appended to `events` in the same transaction
    pub effects: Vector<Effect>,      // performed after commit, results fed back as inputs
    pub reply: R,
}

// One entry point per command; no `&mut`, no handles, no trait objects.
pub fn complete(slice: &TaskSlice, cmd: &CompleteTask, stamp: &Stamp) -> Result<Decision<TaskOut>, WgError>;
```

* **Reads inside the transaction.** `reads`, the load and `decide` run inside the same
  transaction as `apply`, exactly as the Python services do today. `decide` is pure and fast, so
  holding the write lock while it runs costs nothing measurable, and there are no retries or
  optimistic-conflict loops that Python does not have.
* **Ids** are derived from `Stamp.entropy` (UUIDv4 layout, version and variant bits set), so the
  same stamp gives the same ids; the shell draws the entropy from the OS.
* **Slices** are values of persistent collections, so a decision that makes several changes (accept
  a task, then promote its dependents, then release stack children) threads new slice values through
  helper functions without copying.
* **Queries** are pure functions over loaded rows: `list_tasks(&TaskSlice, &TaskFilter) -> Page`.
  The store narrows the rows with indexed SQL (`WHERE workspace_id = ? AND status IN (...)`) and the
  core applies the rest. Where Python relies on SQL for ordering and pagination, the store keeps that
  SQL verbatim and the core does not re-sort, so tie-breaking is SQLite's, as today (R4).

### 4.2 Long-running flows as state machines

Everything that calls out (git, agents, checks, models, Jira) is a state machine in a `Core` crate:

```rust
pub enum RunInput { Start(StartRun), Prepared(GitResults), AgentEvent(AgentLine), AgentExited(Exit),
                    OutcomeCaptured(GitResults), ChecksDone(CheckResults), Cancelled, LeaseLost }
pub fn run_step(state: RunState, input: &RunInput, slice: &RunSlice, stamp: &Stamp)
    -> Result<(RunState, Decision<()>), WgError>;
```

The runtime (`wg-runtime`) is the only loop: it persists the decision, performs the effects outside
any transaction, and feeds each result back. The same pattern carries git sync, verification runs,
stack integration, stack verification with bisection, stack landing, tracker sync, classification,
hook ingestion and spool replay. The flow state that must survive a crash is already in the database
in Python (attempt status, stack status, job rows and leases), so `RunState` and its peers are
loaded from rows, not kept in memory, and recovery is the existing recovery logic run as pure plans.

### 4.3 Workspace layout

Cargo workspace at the repository root (`Cargo.toml`, `crates/`), beside `src/` and `pyproject.toml`
while both exist, so nothing moves when Python is deleted. Edition 2024, a pinned stable toolchain in
`rust-toolchain.toml`, workspace-level lints (`unsafe_code = "forbid"` except in the two crates that
need FFI or Windows APIs, `clippy::unwrap_used` and `expect_used` denied outside tests).

| Crate | Tier | Responsibility | Main dependencies |
| --- | --- | --- | --- |
| `wg-types` | Core | ids, task keys, `Timestamp` (microsecond UTC), the 23 enums, `Actor`, `Permission` sets, `WgError { code, message, details }`, exit-code and HTTP-status mapping | `uuid` (no `v4` feature), `time` (types only), `serde`, `serde_json` (`Value` for details) |
| `wg-model` | Core | entity values (one struct per table family), slices, `Write`, `EventDraft` and typed event payloads, `Effect` and effect-result inputs, `Stamp`, `Decision` | `wg-types`, `rpds` |
| `wg-events` | Core | event record shape, schema versions and upcasters, the projection used by `events verify` | `wg-model` |
| `wg-tasks` | Core | lifecycle table, dependency graph (cycles with path, satisfaction including the stack-sibling rule, ready/blocked propagation, promote-ready, rollups, graph view), create/update/bulk/archive, claims, complete/result/progress/comments, tracked-task ownership, triage-apply, rename-term | `wg-model` |
| `wg-registry` | Core | workspaces, projects, repositories (identity fingerprint, remote normalisation, checkout discovery, allowed roots, health), reference resolution | `wg-model` |
| `wg-gitcore` | Core | parsers for every git output Workgraph reads, file class/language/module, attribution and confidences, rewrite and cherry-pick reconciliation, churn and final change surface | `wg-model`, `regex` |
| `wg-exec` | Core | attempt state machine, run flow, manifests, worktree and branch planning, stack base selection, change sets, escalation, external attempts, worktree recovery plans | `wg-model`, `wg-gitcore` |
| `wg-stacks` | Core | stack membership, item states, integration queue order and merge plans, conflict handling, verification with regression baselines, bisection, ejection and rebuild, landing (compare-and-swap plan, deferral, release of unverified children) | `wg-model`, `wg-exec`, `wg-verify` |
| `wg-verify` | Core | check configs (no shell unless `allow_shell`), check selection by scope and tier, failure-signature parsers, run outcome, acceptance (automated, manual, external), auto-accept | `wg-model`, `regex` |
| `wg-jobs` | Core | queue semantics (claim selection under resource limits, leases, backoff, dead letter, idempotency), worker liveness, maintenance leadership and plan, retention plan, scheduler | `wg-model` |
| `wg-automation` | Core | rule catalog, matching, templates, preview, depth and rate limits, cron | `wg-model` |
| `wg-intel` | Core | classification prompts and parsing, the heuristic provider, taxonomy, evaluation metrics, search ranking (full-text, semantic, hybrid), FTS query quoting | `wg-model` |
| `wg-telemetry` | Core | hook payload normalisation, redaction, dedupe keys, correlation and binding, transcript and sidecar parsing, timing breakdown | `wg-model`, `regex` |
| `wg-trackers` | Core | Jira issue mapping, status mapping, sync decisions, key handling, missing grace | `wg-model` |
| `wg-experiments` | Core | protocol validation, staircase, planner, trials and grading, statistics (Wilson, logistic fit, bootstrap, Brier, reliability), predictor, contract check, briefs, escape, replay selection | `wg-model`, `wg-pyrand` |
| `wg-analytics` | Core | fact tables built from rows, the 37 metric definitions and the query engine (dimensions, filters, periods), every report | `wg-model` |
| `wg-pyrand` | Pure | CPython-compatible Mersenne Twister: `Random(int)` seeding, `random()`, `getrandbits`, `choice`, `choices` | none |
| `wg-json` | Pure | the three JSON dialects of O01, O06, O10 as `serde_json` formatters (separators, indent, `ensure_ascii`, Python float repr, sorted keys), the timestamp formats | `serde`, `serde_json` |
| `wg-wire` | Pure | DTOs for every response and request (field order equals the Pydantic models), conversions from model values, the error envelope, MCP tool and resource descriptors and the OpenAPI document (embedded from the snapshots) | `wg-model`, `wg-json` |
| `wg-validate` | Pure | request and tool-argument validation that produces Pydantic-shaped error lists (`type`, `loc`, `msg`, `input`, `ctx`) | `wg-json` |
| `wg-render` | Pure | rich-compatible tables, key-value blocks, markup handling and colour rules for human output | `unicode-width` |
| `wg-config` | Pure | configuration model, TOML parsing, unknown-key rejection, v1 to v2 migration, workspace merge, `config set` paths, a `tomli_w`-compatible writer, the platformdirs rules as a function of OS and environment | `toml`, `wg-validate` |
| `wg-store` | Impure | rusqlite connection setup, migrations compatible with `alembic_version`, row codecs, one module of SQL per area, slice loading, decision application, FTS5 maintenance, online backup | `rusqlite` (`bundled`) |
| `wg-git` | Impure | the git CLI runner (argument vectors, the E24 environment, timeouts) and one function per git operation, returning text for `wg-gitcore` to parse | `wg-proc` |
| `wg-proc` | Impure | process spawning with a minimal environment, stdin feeding, bounded capture, streaming to log files, timeouts, whole-tree termination (Job Objects on Windows, process groups on POSIX) | `windows-sys`, `libc` |
| `wg-agents` | Impure | the Claude Code adapter (stream-json) and the `workgraph.agent.v1` command adapter, availability checks | `wg-proc` |
| `wg-net` | Impure | blocking HTTP clients: Ollama, OpenAI-compatible, Jira (retries and backoff as configured) | `ureq` |
| `wg-codeparse` | Impure | tree-sitter symbol extraction for Python, C#, TypeScript/TSX, JavaScript, Go, Rust | `tree-sitter` and grammar crates pinned to the Python grammar versions |
| `wg-archive` | Impure | backup and restore zips, JSON Lines export and import, CSV, JSON and Parquet analytics export | `zip`, `parquet`, `arrow` |
| `wg-browser` | Impure | locating Chrome/Edge/Chromium and taking CDP screenshots | `tungstenite` |
| `wg-runtime` | Impure | the effect interpreter, stamping, the load-decide-apply cycle, flow drivers, worker threads, the maintenance leader, JSON logging | `tracing`, `tracing-subscriber` |
| `wg-server` | Impure | axum HTTP API, security middleware (bearer token, Host and Origin checks, headers), LAN listener and pairing, SSE, static UI files, Swagger page, mounting `/mcp/` | `axum`, `tokio`, `rust-embed` |
| `wg-mcp` | Impure | MCP server over stdio and Streamable HTTP, tools and resources registered from the snapshot descriptors; `--grant` permissions apply to both transports (WG-3734) | `rmcp`, `tokio` |
| `wg-cli` | Impure | the `workgraph` binary: hook fast path, `--json` stripping, clap command tree, one handler module per group | `clap` |
| `wg-demo-agent` | Impure | the `workgraph-demo-agent` binary: the scripted `workgraph.agent.v1` agent and the demo checks (DV4) | `wg-proc` |
| `arch-lint` | Tooling | purity tiers and the `Core` rules (copied from noether, section 4.6) | `syn` |
| `wg-conformance` | Tooling | the differential runner, normaliser, drivers, database dump, coverage report | `serde_json`, `rusqlite` |

Section 4.16 (D15) supersedes the per-layer pure/shell crate pairs that grew beside this table
(`wg-runtimespec`, `wg-storecodec`, `wg-clispec`, `wg-serverspec`, `wg-mcpspec`): feature code moves
into the feature crates, which become the unit of ownership from the store to the interfaces.

Dependency direction: `Core` crates depend only on `Core` crates and the allowlist; `Pure` crates on
`Core` and `Pure`; shell crates on anything below them; `wg-cli`, `wg-server` and `wg-mcp` on
`wg-runtime`. No `Core` or `Pure` crate depends on a shell crate. The three interfaces never contain
business rules: they parse, call one runtime entry point, and render (the same rule as
`docs/architecture.md` today).

### 4.4 Effects as data

```rust
pub enum Effect {
    Git(GitOp),                    // worktree add/remove, merge, reset, update-ref, rev-list, log, diff, show, status
    Spawn(ProcessSpec),            // agents and checks: argv, cwd, env, stdin, timeout, log artifact
    Http(HttpCall),                // model providers, Jira
    Fs(FsOp),                      // atomic file writes, artifact files, spool records, worktree directories
    Wake(JobWake),                 // nudge local workers after an enqueue that the decision wrote
    Publish(StreamEvent),          // SSE subscribers in this process
}
```

Each effect has a result type that comes back as a flow input. Enqueuing a job is not an effect: it
is a `Write` of a `jobs` row in the same transaction, as in Python. Logging is not an effect either:
the runtime logs what it does with effects and decisions.

### 4.5 Time, ids and randomness

* The shell reads the clock once per decision (`Timestamp` truncated to microseconds, as Python's
  `datetime` is) and draws 32 bytes of entropy once per decision.
* Seeded statistical randomness (`random.Random(seed)` in the planner and predictor) is reproduced
  bit for bit by `wg-pyrand`, so bootstrap samples and planned trials equal Python's.
* The API token (`secrets.token_urlsafe(32)`) and pairing code use the OS RNG in the shell.

### 4.6 Purity tiers and the lint gate

**Copy, not share.** `C:\dev\noether\crates\arch-lint` (5,565 lines: `purity.rs` 1,726,
`core_tier.rs` 665, `lib.rs` 3,130 of noether-specific architecture lints) is copied into
`crates/arch-lint` with only `purity.rs` and `core_tier.rs` and the `purity-report` command. Reasons:
the two repositories have separate workspaces and release cadences; the `Core` rules hard-code
noether's world types (`TenantWorld`, `PlatformWorld`, `Node`), `noether-tenant` and its dependency
allowlist; a git dependency would tie Workgraph's build to noether's churn. The copy is
parameterised so a later extraction into a shared crate is mechanical (Q5).

Tiers, read from `tools/reports/purity-tiers.txt` exactly as in noether (`Pure`, `StrictPure`,
`Core`; unlisted crates fail the gate; `Tooling` marks). The `Core` rules come from
`tools/reports/core-rules.toml`:

```toml
world_roots = ["TaskSlice", "RegistrySlice", "RunSlice", "RunState", "StackSlice", "StackFlow",
               "VerifySlice", "QueueSlice", "AutomationSlice", "IntelSlice", "TelemetrySlice",
               "TrackerSlice", "ExperimentSlice", "AnalyticsInput"]
external_deps = ["rpds", "serde", "serde_json", "uuid", "time", "regex"]
banned_calls = ["OffsetDateTime::now_utc", "SystemTime::now", "Instant::now", "Uuid::new_v4"]
```

* `Core`: StrictPure plus no `dyn`, no `pub trait`, persistent collections only in types reachable
  from a world root, the dependency allowlist above, and the banned calls (clock and entropy reach
  the core only through `Stamp`). Regexes are compiled where they are used, since a `static`
  `LazyLock` is interior mutability; the patterns involved run over check logs and transcripts, not
  per row.
* `Pure`: noether's semantic rules (no `&mut` in public signatures, no interior mutability, no
  `std::{time,env,fs,net,process,thread}`, no `unwrap`/`expect`).
* Shell crates are `Impure`. The gate keeps noether's **shell logic ceiling**: `cargo purity
  --candidates` over the shell crates must stay at or below the first measured value, so logic that
  could be pure cannot settle in the shell.
* Value tests from noether's §2.9 that apply: every slice and flow state is `Clone + Eq + Debug +
  Send + Sync + 'static` (compile-time assertion), clone isolation and replay determinism property
  tests per Core crate (the same decision twice from the same slice and stamp gives equal results).

### 4.7 Persistence

* **rusqlite, not sqlx.** The store is synchronous (as Python's is), needs exact control of
  `BEGIN IMMEDIATE`, per-connection pragmas, FTS5 and the online backup API, and must not pull async
  into every caller. sqlx's compile-time query checking needs a database at build time and makes the
  store async. rusqlite with the `bundled` feature ships SQLite with FTS5 and JSON functions.
* **Connections.** Same pragmas as `persistence/db.py`: WAL, `busy_timeout=15000`,
  `foreign_keys=ON`, `synchronous=NORMAL`; writes `BEGIN IMMEDIATE`, reads `BEGIN DEFERRED`; one
  connection per thread, no pool.
* **Migrations.** The runner reads and writes Alembic's `alembic_version` table with the same
  revision ids, so either implementation can open a database the other migrated. Revisions `0001` to
  `0011` are ported as SQL whose resulting `sqlite_master` text equals the snapshot byte for byte
  (CF-05 records it per revision), and `0009`'s data migration is ported as code. Downgrades are not
  ported: no command performs one.
* **Row codecs.** Timestamps as O11, JSON columns as O10 (sorted keys, compact), booleans as SQLite
  integers, enums as their string values.
* **Schema changes.** A migration added while both implementations are in use is ported to `wg-store`
  in the same stack, with its `schema-NNNN.sql` snapshot and fixture database (D9).

### 4.8 HTTP server

axum on tokio. The OpenAPI document is not generated: it is the CF-01 snapshot embedded in the
binary and served at `/api/openapi.json`, and a test checks that every route the router registers is
in it with the same method, parameters and status code, and vice versa. Request bodies and query
parameters are validated by `wg-validate` to produce FastAPI's 422 bodies. The Swagger page is
FastAPI's `get_swagger_ui_html` output, reproduced as a constant (it loads swagger-ui from the CDN,
as today). Static UI files are embedded at build time with `rust-embed` from the Vite output
directory (debug builds read the directory), `index.html` gets the token meta tag, and the
placeholder page is served when the UI was not built. The security middleware is a tower layer that
reproduces `api/security.py`: loopback and LAN listeners, `lan_networks`, pairing with an 8-character
code and lockout after 10 wrong codes, the device cookie derived from the token, constant-time token
comparison, Host and Origin checks (421 and 403), the headers of O07, no CORS. SSE uses axum's `Sse`
with the event names and the 15-second ping of sse-starlette.

### 4.9 MCP server

`rmcp`, the official Rust SDK, for stdio and Streamable HTTP (mounted at `/mcp/` under `serve`, or
standalone with `mcp --transport http`). Tools are registered with the input and output schemas
from the CF-03 snapshot rather than derived from Rust types, so the schemas are identical by
construction; `wg-validate` validates arguments against the same rules Pydantic applied. Each tool
handler converts arguments to a command, calls the runtime and returns the DTO as structured content.
Errors become tool errors with the envelope text. Server name, title, version and instructions come
from the snapshot. Protocol-version negotiation and DNS-rebinding behaviour are compared against the
recorded transcripts (R11).

### 4.10 CLI

clap 4 builder API. A test walks the clap command tree and compares it with `contracts/cli.json`:
every command, argument, option name and alias, type, multiplicity, default, environment variable,
`--x/--no-x` pair and help string. `--json` is removed from argv wherever it appears before parsing,
as Python does. `claude hook` is dispatched before clap is built, with nothing but the paths, the
spool writer and the ingestion decision on its path (O15). Handlers render with `wg-json` for
`--json` and `wg-render` for humans, and map `WgError` to O02, O04 and O05.

### 4.11 Async boundaries

tokio exists only in `wg-server` and `wg-mcp` and is started only by `serve` and `mcp`. Everything
below them is synchronous: the store, git, processes, HTTP clients and the runtime. The server calls
the runtime through `spawn_blocking`. Background workers are OS threads, as in Python. Long
subprocesses stream their output on a reader thread per pipe.

### 4.12 Errors

* `Core` and `Pure` crates return `Result<_, WgError>`, a value with the 12 codes of the inventory, a
  message and ordered details. Its exit code and HTTP status are pure functions of the code.
* Shell crates define their own `thiserror` enums (`StoreError`, `GitError`, `ProcError`,
  `NetError`) and convert them into `WgError` at the runtime boundary with the codes Python uses
  (`git_error`, `provider_unavailable`, `resource_busy`, ...). No `anyhow` in libraries; the binary's
  `main` maps any remaining error to exit 1 with the generic message.
* No panics in library code (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing` denied
  outside tests). A panic in a worker thread is caught at the thread boundary and recorded as a
  failed job, as an exception is in Python.

### 4.13 Configuration and logging

`wg-config` parses with `toml` into a `toml::Value`, validates against the model, rejecting unknown
keys with Pydantic-shaped messages, migrates v1 documents, merges workspace files, and writes with a
writer that reproduces `tomli_w`'s output for the shapes Workgraph writes (GV-11 pins it). Paths
follow platformdirs' rules exactly (on Windows `%LOCALAPPDATA%\workgraph` for config and data and
`...\Logs` for logs), computed by a pure function of the OS and an environment snapshot. Logging uses
`tracing` with a formatter that writes the O14 JSON lines to stderr and to a rotating file in
`<logs>`.

### 4.14 Analytics without DuckDB (decision D12)

Python copies tables into an in-memory DuckDB database and runs SQL. The Rust version loads the same
columns (`analytics/warehouse.py` `TABLES`) through `wg-store` into values and computes the fact
tables (`task_facts`, `attempt_facts`, `task_commits`, `commit_facts`, `task_files`) and the 37
metrics as pure functions in `wg-analytics`. DuckDB's semantics that matter are reproduced and pinned
by GV-15: `quantile_cont` and `median` (linear interpolation), `date_trunc('week')` (Monday),
`epoch()`, `AVG` over integers, `COUNT(DISTINCT ...)`, NULL handling, and the `ROW_NUMBER() OVER`
dedupe by patch id. This keeps analytics in the pure core (the user's requirement), removes a large
native dependency that crashes uvicorn on CPython 3.12 for Windows today (`pyproject.toml` comment),
and DV3 records the missing cache file.

### 4.15 Library choices

| Need | Choice | Reason |
| --- | --- | --- |
| SQLite | `rusqlite` (`bundled`) | sync, FTS5 and JSON1 included, exact transaction control, no build-time database |
| HTTP server | `axum` + `tokio` | tower middleware for the security layer, SSE support, used by `rmcp`'s HTTP transport |
| MCP | `rmcp` | the official Rust SDK, stdio and Streamable HTTP |
| CLI | `clap` 4 (builder) | full control of names, aliases, env fallbacks and `--x/--no-x` pairs |
| HTTP client | `ureq` | blocking, small, no async runtime in the core paths |
| JSON | `serde` + `serde_json` with custom formatters | exact Python dialects without a bespoke parser |
| TOML | `toml` | parsing; writing is `wg-config`'s own `tomli_w`-compatible writer |
| Persistent collections | `rpds` | the same as noether; O(1) clone, structural sharing |
| Time and ids | `time`, `uuid` | value types only in the core |
| Processes on Windows | `windows-sys` (Job Objects) | kill whole trees as `taskkill /T` does |
| Tree-sitter | `tree-sitter` + grammar crates pinned to the Python grammar versions | identical symbol extraction (R17) |
| Archives and Parquet | `zip`, `arrow` + `parquet` | backup and export formats |
| Static assets | `rust-embed` | single-binary install |
| Logging | `tracing`, `tracing-subscriber`, `tracing-appender` | JSON lines and rotation |

---

### 4.16 Feature crates (decision D15, 2026-10-26)

**Problem.** The crates in 4.3 that sit between the domain crates and the binaries grew sideways
during implementation: `wg-runtimespec` (Pure) and `wg-runtime` (Shell), `wg-storecodec` and
`wg-store`, `wg-clispec` and `wg-cli`, `wg-serverspec` and `wg-server`, `wg-mcpspec` and `wg-mcp`.
Each has one module per feature (`automation`, `decisions`, `git`, `jobs`, `repos`, `stacks`,
`tasks`, `worktrees`, ...), so one feature is spread over up to eight crates. WG-3563 (`automation`)
touched `wg-automation`, `wg-jobs`, `wg-storecodec`, `wg-store`, `wg-runtimespec`, `wg-runtime`,
`wg-clispec` and `wg-cli`. The pairs exist only because purity tiers are assigned per crate, so the
pure half and the `&mut` half of every layer need separate crates. The registry lists every feature
edits (`GROUPS`, the `IMPLEMENTED` lists, route and tool tables, `wg-runtime` re-exports) caused
most stack conflicts, and one feature was implemented twice in different layers (WG-3772's
`land_stack` beside `acceptance.rs`).

**Shape.** A feature owns its whole vertical slice in at most two crates (amended 2026-10-26, see
"Core crate and feature crate" below): its domain core crate from 4.3 (`wg-tasks`, `wg-registry`,
...), which stays pure, and a feature crate `wg-<area>-feature` for everything that needs
`wg-runtime`. A feature whose logic no other code uses keeps both in one crate (the pilot,
`wg-decisions`). Inside a feature crate:

| Module | Tier | Holds |
| --- | --- | --- |
| `core` (and submodules) | Core | the decisions and queries over slices (what the domain crate holds today) |
| `spec` | Pure | request parsing, reply shaping, the feature's DTOs and its SQL statement text (today's per-feature modules of `wg-runtimespec`, `wg-storecodec`, `wg-wire`) |
| `service` | Shell | the load-decide-apply glue: free functions over `&mut App` / `&mut Session` (today's per-feature modules of `wg-runtime` and `wg-store`), under the shell logic ceiling |
| `cli`, `http`, `mcp` | Shell | the feature's clap group, routes and tools, each a table of `fn` pointers into `service` (today's per-feature modules of `wg-clispec`, `wg-cli`, `wg-serverspec`, `wg-server`, `wg-mcp`) |
| `jobs` | Shell | the feature's job handlers and maintenance steps |

Each feature crate exports one value:

```rust
pub static FEATURE: Feature = Feature {
    name: "decisions",
    cli: &[cli::GROUP],          // clap builder + handler, as `wg_cli::commands::Group` today
    routes: &[http::ROUTES],     // `RouteSpec` + handler, as `wg_server::groups` today
    tools: &[mcp::TOOLS],        // descriptor name + handler
    resources: &[],
    jobs: &[],                   // job kind + handler
};
```

`Feature` and the handler signatures live in `wg-runtime`, so a feature depends on it and never on
a front end. `wg-cli`, `wg-server`, `wg-mcp` and the worker are generic: they take `&[&Feature]`
and keep only what is not a feature (argv handling, `--json` stripping, security middleware,
transports, the worker loop). The binary holds the one ordered list `FEATURES`. Order is explicit
rather than distributed registration (`linkme`, `inventory`) because CLI help order and route order
are contracts, and the snapshot tests compare them. The `IMPLEMENTED` lists of the clap-tree, route
and tool coverage tests are derived from `FEATURES`, not maintained by hand.

**Base crates** stay horizontal and contain no feature code: `wg-types`, `wg-model`, `wg-events`,
`wg-json`, `wg-validate`, `wg-render`, `wg-config`, `wg-wire` (envelope, conventions, shared DTOs),
`wg-store` (connection, migrations, session, codecs, query building; every migration stays here,
since the schema is one database), `wg-runtime` (`App`, stamping, the cycle, effects, bus, worker
loop, logging, `Feature`), and the effect adapters (`wg-git`, `wg-proc`, `wg-net`, `wg-agents`,
`wg-codeparse`, `wg-archive`, `wg-browser`).

**Purity by module.** `tools/reports/purity-tiers.txt` accepts `crate::module Tier` lines beside
crate lines. A feature crate lists every top-level module; an unlisted module fails the gate, as an
unlisted crate does. A `Core` module may only refer to `Core` crates, the allowlist and other `Core`
modules (arch-lint checks `use` items and paths, not just `Cargo.toml`). The shell logic ceiling is
measured over the `Shell` modules. The pure/shell crate pairs then have no reason to exist.

**Between features.** Feature crates form a DAG: `decisions` depends on `tasks`, `stacks` on
`tasks`, `exec` and `verify`, and so on. A feature calls a lower feature's `service` or `core`
functions; no cycles. Where Python reacts to another area through events or jobs, Rust does too.

**What adding a feature touches.** Its own crate, one line in `FEATURES`, and `purity-tiers.txt`.

**Core crate and feature crate (amended 2026-10-26).** The first moves after the pilot (FC-04
tasks, FC-05 registry) showed that a domain crate cannot also be its feature crate. `wg-runtime`,
`wg-runtimespec`, other features and base crates use the task and registry cores (`wg_tasks::
transition`, `deps`, `rename`; `wg_registry::resolve`, `checkout`, `boundary`, `remote`,
`identity`), and a feature crate must depend on `wg-runtime` for `Feature` and `App`, so putting
the feature in the domain crate is a Cargo cycle. The rules:

1. **Two crates at most.** The domain core crate (`wg-<area>`, Core tier, depends on no shell
   crate) and the feature crate (`wg-<area>-feature`: `spec`, `service`, `cli`, `http`, `mcp`,
   `jobs`, the `FEATURE` static; depends on the core crate and `wg-runtime`). Adding a feature is
   still one new crate and one line in `FEATURES` when its core crate exists.
2. **Shared code sits below the feature crates.** Anything that base code or another feature
   uses goes in the owning core crate if it is pure (task acceptance and release decisions, which
   stack landing uses, belong in `wg-tasks`; reference resolution stays in `wg-registry`), or stays
   in base if it is not: row readers used by several features (`task_by_key`, `tasks_by_key`,
   `task_files`, `index_task`, `task_world`, `fts_task_ids`) stay in `wg-store`, and the generic
   clap argument readers of `wg-clispec::task` (`get`, `items`, `one`, `many`, `number`, `flag`,
   `option_*`, ...) move to a base CLI helper module, not into a feature.
3. **Base never depends on a feature crate.** Feature crates may depend on other feature crates
   (a DAG: stacks on tasks, decisions on tasks), but `wg-runtime`, `wg-store`, the front ends'
   base parts and every core crate do not. Only the binary's `FEATURES` list names them all.
4. **Order of a move.** A feature's code leaves the layer crates once nothing left there needs it.
   A move first lowers the shared pieces (rule 2), then moves the rest. Where base code still
   holds another feature's code that uses this feature's impure parts (stack landing in
   `wg-runtime/acceptance.rs` uses task services), that use is first rewritten against the core
   crate or left in place and listed for the later move.

**Pilot: `wg-decisions` (WG-3781).** The decisions feature is the first crate of this shape. What
it changed from the text above:

* **Modules as built.** `core.rs` (Core: the former `wg_tasks::decisions` plans, moved whole, so
  `wg-tasks` no longer has a decisions module); `spec/` (Pure: `args.rs` holds the clap command
  and the argument readers, since they are functions of `ArgMatches`; `dto/` the DTOs; `sql.rs`
  the statement text and row decoding; `view.rs` the human views and the prefix lookup;
  `shapes.rs` the output shapes of the feature's tools); `service/` (Shell: `mod.rs` the `App`
  entry points, `reads.rs` the `Session` readers); `cli.rs`, `http.rs`, `mcp.rs` (Shell: the
  handler, the route table, the tool table). The `jobs` table is empty and has no module yet.
* **Reach.** `App` is a type of `wg-runtime`, and an inherent impl cannot be written in another
  crate, so the service is free functions over `&mut App`, and the readers free functions over
  `&mut Session`. Each one is named after its Python method and called by the feature's own
  tables. Three `App` helpers and `index_written` became `pub` in `wg-runtime` for that.
* **Descriptor tables.** `FEATURE.cli` is `[cli::GROUP]`, `FEATURE.routes` is `[http::GROUP]` (a
  `RouteGroup` over `http::SPECS`), `FEATURE.tools` is `mcp::TOOLS`, one `ToolRun` that looks the
  name up in `mcp::TABLE` (the table of names and bodies).
* **Front ends.** `wg-cli` lists the feature in `commands::FEATURES`; the decisions group left
  `GROUPS`. `wg-server` gets its routes from `routes_with(FEATURES)`, and `wg-mcp` answers the
  feature's tools through `AppBackend.features`. The standalone `wg-mcp` program and its test pass
  a static list holding the same feature; a later task should let that program read the binary's
  list rather than name the feature itself.
* **Route order.** The decision routes now register after the groups that predate the descriptor,
  so they come after the stacks routes instead of between events and tasks. The route-table test
  matches by operation id, and no decision path collides with another route.
* **Output shapes.** The wire catalog (`wg_wire::dto::mcp_outputs`) keeps the shapes of the other
  tools and no longer holds the decision ones; `wg3738_wire.rs` leaves the three decision tools
  out of its catalog checks. Moving the catalog check to the features (a `shapes` table on the
  descriptor, checked in `wg-cli`) is a follow-up, because the descriptor is shared.
* **Shared helpers.** `text_param` is now a root function of `wg-runtimespec` (the server's stacks
  routes read path parameters with it), and the task argument helpers of `wg-clispec::task` are
  `pub`, so the clap readers can use them.
* **Purity.** Module tiers are used for the first time: `wg-decisions` is a Shell crate with
  `wg-decisions::core Core`, `wg-decisions::spec Pure`, and `service`, `cli`, `http`, `mcp` Shell.
  The shell pure-candidate ceiling holds at 2 lines with the feature's argument readers in `spec`.
* **Not moved.** The row codec of `task_decisions` (`wg-storecodec` `rows_more`), the entity
  `TaskDecision` and the commit bundle `wg_model::Decision` stay in the base crates, as do the
  `/api` inventory rows H125 to H130 in `wg-serverspec::operations`, the MCP catalog entries in
  `wg-mcpspec`, and the `ui open decisions` spec in `wg-uispec`.

## 5. Migration and cut-over

### 5.1 Coexistence rules

* **One database.** Both binaries open the same `%LOCALAPPDATA%\workgraph` home. A schema change is
  implemented in both versions in one stack (D9); `0011` (`task_decisions`) is the first.
* **Shared queue.** Job rows, payloads, idempotency keys, resource keys and leases have the same
  shapes in both, so a Python worker and a Rust worker can drain one queue (VF-04). Hook spool
  records are readable by both.
* **Contract drift.** The CF-01 drift test runs in the Python suite. A Python change that alters a
  contract updates the snapshot in the same commit and gets a paired Rust task (Q3).
* **Web build output.** Vite writes to `src/workgraph/api/static` today, and both servers read it
  there while both exist. At retirement the output directory moves (Q7).

### 5.2 Phases by dependency

1. **Foundations** (sections 9.3 to 9.5): workspace, lint gate, contract snapshots, golden vectors,
   the conformance runner, the Python harness switch. Nothing user-visible.
2. **Core and support crates** (9.6, 9.7): types, model, JSON, rendering, validation, config, every
   `Core` crate against its golden vectors, the store and migrations, the shell adapters, the runtime.
3. **Interfaces** (9.8, 9.9): CLI groups, MCP tool groups, HTTP route groups, hook path, installer.
4. **Parity** (9.10): Python interface tests in Rust mode, the full differential corpus, database
   round trips and migrations, mixed queue, measured speed comparison.
5. **Switch and retirement** (9.11): side-by-side install, switch, retirement, documentation.

### 5.3 Installing, switching and rolling back

* **Side by side.** `cargo install --path crates/wg-cli --root %USERPROFILE%\.local` with the binary
  renamed `workgraph-rs.exe` (a cargo `[[bin]]` alias used only for this phase). The coordinator
  can point read-only work at it while `workgraph` stays Python.
* **Switch.** `uv tool uninstall workgraph`, then install the Rust binary as `workgraph.exe` in
  `%USERPROFILE%\.local\bin`. The user-scope MCP registration
  (`C:/Users/Brice/.local/bin/workgraph.exe mcp --agent-name claude-code`) and the Claude Code hook
  handlers (which store the resolved `workgraph` executable path, F12) keep working because the path
  is the same. `workgraph claude status` and `workgraph doctor` confirm.
* **Rollback.** `uv tool install .` from the Python checkout restores the Python command at the same
  path. Because the schema and every row format are shared, no data step is needed in either
  direction.

### 5.4 Retiring Python

When VF-02 reports zero failed and the user ends the soak: delete `src/workgraph`, `pyproject.toml`,
`uv.lock`, the Python tests and `tools/*.py` that only served Python; keep `golden/`, `contracts/`,
`tests/conformance/` (scenarios and recorded goldens) and switch the runner to `check` mode only;
point Vite's `outDir` at the directory `wg-server` embeds; rewrite the docs that describe Python
internals (`architecture.md`, `testing.md`, `getting-started.md`, `extending.md`, `README.md`). The
contract snapshots stay as the reference the Rust tests compare against.

### 5.5 Keeping the repository green

* Every change runs the cheap checks only: `cargo check` on the touched crates, `cargo fmt --check`,
  `cargo clippy -p <crate> -- -D warnings`, `cargo purity` (gate), and for Python-side tasks
  `ruff check` on the touched files. No task runs test suites on its own change.
* Test suites run once per stack, on the stack's combined result: `uv run pytest -n auto` (Python,
  unchanged), `cargo test --workspace`, and once the interface skeletons exist the conformance
  runner over the scenarios the stack touches (the whole corpus from the parity stage on).
* Contract-coverage tests (the clap tree, the OpenAPI route table, the DTO field orders) check only
  the groups listed as implemented in the test itself; each interface or wire task adds its group to
  that list. Unimplemented groups are reported, never failed, so the gate stays green while the
  rewrite is partial.
* Workgraph's own verification configuration for this repository needs the Rust checks added as
  trusted configuration (Q10); until then the stack checks above are run by the coordinator.
* Python stays the installed command until the switch, so a broken Rust change never affects users.

---

## 6. Risks

| Id | Risk | Why it is hard | Handling |
| --- | --- | --- | --- |
| R1 | rich table layout | column widths from content and terminal width, folding, box characters, the dim "nothing to show" line, markup in interpolated text | `wg-render` reimplements the subset Workgraph uses; GV-13 pins tables of many shapes and widths; human output compared at `COLUMNS=120` with stdout not a terminal (LIB-07 is `Tier: mid`) |
| R2 | Typer/Click help and usage errors | rich panels | content checked against the snapshot; layout is DV1 (Q1) |
| R3 | JSON byte equality | three dialects; `ensure_ascii`; Python float repr (`1e-05`, `1e+16`, `1.0`); Pydantic field order; three timestamp formats (`Z`, `+00:00`, DB); `default=str` (a nested `datetime` becomes `2026-10-25 13:56:00+00:00`); `allow_nan=False` on HTTP | `wg-json` with GV-12; DTO field order checked against the OpenAPI component schemas; each DTO field carries its timestamp format explicitly |
| R4 | Ordering | SQL `ORDER BY` ties, Python's stable sorts, dict insertion order | the store keeps Python's SQL ordering verbatim; core sorts are stable and use the same keys; scenario corpus compares lists in order |
| R5 | Floats in analytics and statistics | DuckDB `quantile_cont`, `AVG`, rounding (`round()` is half-to-even), summation order, libm differences | pure Rust with the same operation order; GV-15 and GV-05 vectors; exact comparison except DV7 fields |
| R6 | FTS5 and bm25 | tokenizer `porter unicode61`, ranking ties, the bundled SQLite version differs from CPython's | same tokenizer declaration; result order compared in scenarios; a store test asserts FTS5, `bm25` and the JSON functions are available in the bundled SQLite |
| R7 | Pydantic validation messages | 422 bodies and MCP argument errors list Pydantic's `type`, `loc`, `msg`, `input`, `ctx` | `wg-validate` covers the field types the DTOs use; GV-14 pins messages (LIB-06 is `Tier: mid`) |
| R8 | Windows paths | `Path.resolve()` versus `canonicalize` (`\\?\` prefixes), case, separators in stored checkout paths | paths normalised with `dunce`-style rules; GV-18 and scenario checks on stored paths |
| R9 | Windows processes | tree kill, console encoding, `PATHEXT` resolution, Job Objects inside `uv`-launched shells | `wg-proc` (`Tier: mid`) with tests that spawn grandchildren and kill the tree; DV2 for console output |
| R10 | platformdirs | the `directories` crate disagrees with platformdirs on Windows and Linux | `wg-config` implements platformdirs' rules directly (GV-18) |
| R11 | MCP SDK differences | protocol version negotiation, structured-output wrapping, error shapes, session handling and DNS-rebinding responses in Streamable HTTP | CF-03 transcripts replayed against Rust; descriptors served from the snapshot |
| R12 | FastAPI edge behaviour | 405 bodies `{"detail": ...}`, trailing-slash redirects, `HEAD`, query parsing of lists and booleans | CF-04 records them; LIB-05 and IF-40 reproduce them |
| R13 | Seeded randomness | the planner and predictor use `random.Random(seed)` | `wg-pyrand`, GV-06 |
| R14 | Timestamps | microsecond truncation; Windows clock resolution; `isoformat()` drops zero microseconds | `Timestamp` type with explicit formatters; GV-12 |
| R15 | Concurrency with Python | both implementations writing one database and one queue | identical transaction discipline and row formats; VF-04 |
| R16 | Stack flows | `integrations/git/stack_*` and `core/services/stacks.py` are 2,988 lines with the most branching in the code base and a 1,389-line test file | split into membership, queue, verification and landing tasks; the verification and bisection task is `Tier: top`; SC-05 to SC-08 port every case of `test_stack_integration.py` |
| R17 | Tree-sitter grammar versions | different grammar versions give different symbols | pin the Rust grammar crates to the versions in `uv.lock`; GV-16 |
| R18 | `tomli_w` output | `config set` and `config migrate` rewrite files | writer pinned by GV-11 |
| R19 | Startup on the hook path | Claude Code runs `claude hook` on every event | the hook path is dispatched before clap; VF-05 measures it against Python |
| R20 | Contract drift during the rewrite | Workgraph is under active development in Python | drift test (CF-01), paired tasks (Q3) |
| R21 | Haiku agents and a hub interface | most tasks depend on `wg-model`'s conventions | LIB-04 (`Tier: top`) fixes the conventions and the task-domain types first, with worked examples the later model tasks copy |

---

## 7. Decisions

| Id | Decision |
| --- | --- |
| D1 | Ground-up rewrite as a Cargo workspace at the repository root, beside Python until retirement |
| D2 | One decision over one loaded slice is the unit of purity; slices are `rpds` values; the decision runs inside the transaction, effects run after commit |
| D3 | Long flows are pure state machines driven by one runtime loop; their durable state is the existing rows |
| D4 | Copy noether's `arch-lint` purity checker and parameterise its `Core` rules with `tools/reports/core-rules.toml` |
| D5 | rusqlite (bundled), synchronous store; tokio only in `wg-server` and `wg-mcp` |
| D6 | The OpenAPI document, the MCP descriptors and the CLI tree are served or checked from the Python snapshots, not regenerated from Rust types |
| D7 | Parity is measured by a Rust differential runner over a language-neutral scenario corpus, plus golden vectors, plus the Python interface tests in Rust mode |
| D8 | Normalised comparison with consistent placeholders; timestamps compared by rank |
| D9 | Schema changes while both implementations are in use land in both in one stack (user decision 2026-10-25, first: `0011` task decisions); Alembic's version table kept |
| D10 | Same install path for the switch; rollback by reinstalling the Python tool |
| D11 | The Python service-level tests are replaced by scenarios and golden vectors, not translated line by line into Rust tests |
| D12 | Analytics computed in pure Rust instead of DuckDB |
| D13 | The demo uses a companion binary `workgraph-demo-agent` for its scripted agent and checks |
| D14 | Python's unit tests of pure functions become golden vectors and Rust unit tests in the crate that owns the logic |
| D15 | Vertical feature crates: each feature owns its core, spec, service and interface modules; purity tiers apply per module; front ends and the worker collect one ordered `FEATURES` list (4.16, user decision 2026-10-26) |

---

## 8. Open questions for the user

1. **Q1 Help layout.** Accept clap-rendered `--help` and usage errors with the same content (DV1),
   or require Typer's rich layout byte for byte? Recommendation: content parity.
2. **Q2 Piped output encoding on Windows.** Accept UTF-8 for piped human output (DV2)?
   Recommendation: yes.
3. **Q3 Contract changes during the rewrite.** Freeze contract changes in Python from the start of
   interface stage (9.8), or allow them with a paired Rust task each time? Recommendation: allow, paired, with the drift
   test as the guard. Also: should a defect found during the rewrite be fixed in both, or reproduced
   in Rust and fixed later?
4. **Q4 Schema freeze.** Decided 2026-10-25: no freeze; a migration is implemented in both versions
   in one stack (D9).
5. **Q5 arch-lint.** Confirm copying it (D4) rather than extracting a crate shared with noether now.
6. **Q6 Swagger UI.** Keep `/api/docs` byte-identical, including its CDN script tags?
   Recommendation: keep.
7. **Q7 Web build directory.** After retirement, Vite's `outDir` has to change from
   `src/workgraph/api/static`. Is a one-line change to `web/vite.config.ts` acceptable under "the web
   UI stays as is"?
8. **Q8 Demo.** Accept DV4 (`workgraph-demo-agent` instead of Python for the demo's agent and checks)?
9. **Q9 Float tolerance.** Accept `1e-12` relative tolerance for statistics computed through
   `exp`/`log` (DV7)?
10. **Q10 Workgraph verification for this repository.** Adding the Rust checks (`cargo fmt`,
    `clippy`, `purity`, `cargo test`, the conformance runner) to Workgraph's verification config for
    the `workgraph` repository needs `config:manage`. Will you add them, or authorise an agent to?
11. **Q11 Python tests after retirement.** Delete them (scenarios, goldens and Rust tests replace
    them), or keep the interface tests runnable against the Rust binary with a minimal Python test
    environment? Recommendation: delete.
12. **Q12 Soak.** What ends the side-by-side phase: a number of stacks landed with the Rust binary
    installed, zero failed corpus runs, or your call?

---

## 9. Task breakdown

### 9.1 Conventions for every task

* The first line of each brief is `Task-ID: <KEY>`. Commits end with the `Task-ID:` trailer; no
  mention of any assistant or model in commits.
* Work in a worktree of the `workgraph` repository, never in the live checkout `C:\dev\workgraph`.
* Cheap checks only (section 5.5). Tests named in the acceptance criteria are written by the task
  and run when the stack is verified, not by the task.
* `Tier: haiku` unless marked `Tier: mid` or `Tier: top`.
* "Vectors pass" means the crate's tests that load `golden/<name>/` assert equality for every
  vector. "Scenarios pass" means the conformance runner in `diff` mode reports no failed step for
  the named scenario files.
* Crate paths are `crates/<crate>/`. Golden vectors live in `golden/<name>/`, scenario files in
  `tests/conformance/scenarios/`.
* Test marker: a comment `covers: C001, M02` on the line above a test function (Rust `fn` or Python
  `def`; attributes and decorators may sit between) makes `wg-conformance coverage` count that test
  as covering the named ids. Rust unit and integration tests and Python tests under `tests/` are read.
* Allowlist: `tests/conformance/coverage-allowlist.toml` holds `[[exclusion]]` tables with `id` and a
  non-empty `reason` for ids that no scenario or test can observe; coverage reports them as excluded
  with the reason, not uncovered.

### 9.2 Dependency levels and parallel width

A task can start as soon as all its dependencies are done. The level of a task is the length of the
longest dependency chain below it, so all tasks of one level can run at once. Computed from the
dependency columns below:

| Level | Tasks | Count |
| --- | --- | --- |
| L0 | FD-01, CF-01, GV-01 to GV-21, TH-01 | 24 |
| L1 | FD-02, CF-02 to CF-06, CR-01, LIB-01, LIB-02, LIB-03, LIB-06, LIB-07, LIB-08, SH-19, TH-02 to TH-07 | 20 |
| L2 | FD-03, LIB-04, LIB-09, CR-02 to CR-07, SH-16, SH-21, SH-22 | 12 |
| L3 | CORE-01, CORE-06, LIB-05, LIB-10, LIB-11, SH-01, SH-18, SC-01 to SC-36 | 43 |
| L4 | CORE-02, CORE-07, CORE-08, CORE-09, CORE-11, CORE-16, CORE-19, CORE-21, CORE-22, CORE-24, CORE-27, LIB-12 to LIB-19, SH-02, SH-04 to SH-14, SH-20, SH-23 | 33 |
| L5 | CORE-03, CORE-04, CORE-10, CORE-12, CORE-13, CORE-14, CORE-20, CORE-25, CORE-26, CORE-28, CORE-29, IF-01, IF-30, IF-40, SH-03, SH-15, SH-17, SH-24, SH-25 | 19 |
| L6 | CORE-05, CORE-15, CORE-23, IF-02, IF-04, IF-05, IF-06, IF-09 to IF-12, IF-14 to IF-23, IF-31 to IF-33, IF-35, IF-36, IF-41 to IF-49, IF-50, IF-51 | 37 |
| L7 | CORE-17, IF-03, IF-07, IF-13 | 4 |
| L8 | CORE-18 | 1 |
| L9 | IF-08, IF-24, IF-34, IF-53 | 4 |
| L10 | VF-01, VF-02 | 2 |
| L11 | VF-03, VF-04, VF-05, CO-02 | 4 |
| L12 to L15 | CO-01, CO-03, CO-04, CO-05, one at a time | 4 |

Total 207: 186 `haiku`, 17 `mid` (FD-02, CR-01, LIB-05, LIB-06, LIB-07, SH-01, SH-02, SH-16,
SH-24, CORE-02, CORE-12, CORE-15, CORE-18, CORE-28, IF-01, IF-30, VF-02), 4 `top` (LIB-04,
CORE-17, SH-23, IF-40).

* The scenario tasks SC-01 to SC-36 are needed only by VF-02, so they can be spread over L3 to L9.
  When L4 starts while they are still running, 76 tasks can be in flight.
* The critical path is the stack chain: FD-01, LIB-01, LIB-04, CORE-01, CORE-02, CORE-14, CORE-15,
  CORE-17 (`top`), CORE-18, IF-24, VF-02, VF-03, CO-01, CO-03, CO-04, CO-05. Everything that does
  not need stack landing (all other interface groups) finishes at L6, so the stack chain is the one
  to staff first and to review most carefully.

### 9.3 Foundations that need nothing

**FD-01 Cargo workspace skeleton**
Touches: root `Cargo.toml`, `rust-toolchain.toml`, `.cargo/config.toml` (aliases `purity`,
`lint-arch`), `rustfmt.toml`, `.gitignore`, one stub crate per row of the table in 4.3 (empty
`lib.rs` or `main.rs` with a crate doc comment), `tools/reports/purity-tiers.txt`,
`tools/reports/core-rules.toml` (contents of 4.6).
Accept: `cargo check --workspace`, `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`
pass; every crate has a tier line; workspace lints as in 4.3; `wg-cli` produces a `workgraph` binary
that prints nothing yet. Deps: none.

**CF-01 Contract snapshots and drift test**
Touches: `tools/contracts/dump.py` (extracts the CLI tree, MCP tools/resources, OpenAPI, `sqlite_master`
at head, config JSON Schema), `contracts/{cli,mcp,openapi,config-schema}.json`, `contracts/schema-0011.sql`,
`tests/unit/test_contract_snapshots.py`.
Accept: counts equal the inventory (192, 69, 2+5, 130/114/81, 73 tables, 211 indexes); the test
passes on `main` and fails when an option is renamed (shown in the task report with a scratch edit
that is not committed); dump output is deterministic. Deps: none.

**GV-01 to GV-21 Golden vectors** (each: a deterministic script `tools/golden/gen_<name>.py` (the `gen_` prefix keeps a script from shadowing the package it vectors, such as `pydantic`, `rich` or `queue`) that writes
`golden/<name>/*.json`; two runs give identical bytes; every file under 2 MB; the report lists the
cases and which Python tests' cases they include). Deps: none.

| Id | Name | Python source | Cases |
| --- | --- | --- | --- |
| GV-01 | `lifecycle` | `core/policies/lifecycle.py` | every (from, to) pair with `privileged` and `tracker` flags, attempt transitions |
| GV-02 | `signatures` | `verification/service.py` `extract_failure_signatures`, `integrations/git/regression.py` | logs from `tests/unit/test_failure_signatures.py`, `test_regression.py`, plus cargo logs with several binaries, build failures, pytest and generic logs |
| GV-03 | `gitparse` | `integrations/git/parse.py`, remote URL normalisation, file class/language/module | outputs of real `git log`/`numstat`/`worktree list --porcelain` from generated repos, URLs of every supported form |
| GV-04 | `attribution` | `integrations/git/attribution.py`, message inference | cases of `tests/unit/test_attribution.py` plus generated trailers and messages |
| GV-05 | `expstats` | `experiments/stats.py`, `staircase.py`, `planner.py`, `predictor.py` (pure parts) | cases of `test_experiment_stats.py`, `test_staircase.py`, generated datasets and seeds |
| GV-06 | `pyrand` | `random.Random` | seeds 0, 1, 2**32, 2**64+5, negative; first 2,000 `random()`, `getrandbits(k)` for k in 1..64, `choice`, `choices` |
| GV-07 | `telemetry` | `security/redaction.py`, `integrations/claude_code/transcript.py`, `timing.py`, hook payload normalisation in `ingest.py` | cases of `test_transcript.py`, `test_transcript_timing.py`, redaction of every secret pattern |
| GV-08 | `jira` | `integrations/trackers/mapper.py`, `core/policies/trackers.py` | cases of `test_tracker_mapper.py`, the fixtures in `tests/fixtures/jira` |
| GV-09 | `intel` | `integrations/llm/heuristic.py`, `classification/taxonomy.py`, FTS query building and hybrid scoring in `search/service.py` | generated titles and descriptions, adversarial FTS input |
| GV-10 | `automation` | `automation/cron.py`, template rendering and rule matching in `automation/rules.py` | cron expressions with expected next times, templates, rule conditions |
| GV-11 | `config` | `config/settings.py` load, validate, migrate, merge, `config set`, `tomli_w` output | valid and invalid documents, every key of contracts section 13.5, v1 documents |
| GV-12 | `jsonfmt` | `json.dumps` in the three dialects, Pydantic `model_dump(mode="json")` timestamps, `isoformat()`, `when()` | strings with every escape class, non-ASCII, floats across magnitudes, NaN rejection, nested `default=str` values |
| GV-13 | `rich` | `cli/common.py` `table`, `kv`, error printing through rich | tables of 1 to 8 columns with long and wide (CJK, emoji) cells at widths 40, 80, 120, 200; empty tables; text containing `[` markup |
| GV-14 | `pydantic` | the request models in `core/models/schemas.py` and the MCP tool signatures | each field type with missing, wrong-type, out-of-range and extra values; the resulting error lists |
| GV-15 | `analytics` | `analytics/warehouse.py`, `metrics.py`, `service.py`, report modules | the demo seed database and two generated databases; every metric by every allowed dimension and period; every report |
| GV-16 | `codeparse` | `analytics/code/structure.py` | before and after sources per language with added, modified and deleted functions and types |
| GV-17 | `expmisc` | `experiments/contract.py`, `briefs.py`, `escape.py`, replay selection in `replay.py` | cases of `test_contract.py`, `test_briefs.py`, `test_escape.py`, generated commit lists |
| GV-18 | `paths` | `config/paths.py` with platformdirs | Windows, Linux and macOS with and without `WORKGRAPH_HOME`, XDG variables set and unset |
| GV-19 | `taskgraph` | `satisfied_dependencies`, cycle detection, `promote_ready`, rollups, `graph` in `core/services/tasks.py` (driven through a temporary Workgraph) | generated graphs with stacks, parked tags, archived and accepted tasks, cycles in dependencies and in parents |
| GV-20 | `queue` | `jobs/queue.py`, `jobs/limits.py` (driven through a temporary database with a fixed clock) | claim order under resource limits, backoff delays, lease expiry, dead letter, idempotency collisions, cancellation |
| GV-21 | `stackplan` | the pure parts of `stacks.py`, `stack_integration.py`, `stack_verification.py`, `stack_landing.py` (queue order, `builds_on_others`, prefix construction for bisection, regression judgement, landing preconditions) | generated stacks with dependency chains, conflicts, base failures, errored checks |

**TH-01 Python harness switch**
Touches: `tests/conftest.py`, `tests/helpers.py`.
Accept: `WORKGRAPH_IMPL` (default `python`) and `WORKGRAPH_RUST_BIN`; helpers `run_cli(args, env)`
(result object with `exit_code`, `stdout`, `stderr`, `output`), `mcp_stdio_params(...)`,
`api_client(app)` that return the Python in-process forms by default and the subprocess forms under
`rust`; the Python suite still passes in `python` mode on the stack. Deps: none.

### 9.4 Foundations on the workspace and snapshots

**FD-02 arch-lint copied and adapted** (`Tier: mid`)
Touches: `crates/arch-lint/src/{main,lib,purity,core_tier}.rs`.
Accept: `purity.rs` and `core_tier.rs` copied from noether with attribution to the source path in a
module comment; world roots, dependency allowlist and banned calls read from
`tools/reports/core-rules.toml`; noether's fixtures adapted (world type names from the rules file);
`cargo purity` prints tiers and LOC and fails on an unlisted crate; the shell logic ceiling file
`tools/reports/purity-baseline.txt` created. Deps: FD-01.

**CF-02 Contracts document generated**: `tools/contracts/render_md.py` regenerates section 13 of
`docs/rust-rewrite-contracts.md` from `contracts/`; output equals the committed section. Deps: CF-01.

**CF-03 MCP transcripts**: `tools/contracts/record_mcp.py`, `contracts/mcp/*.jsonl` (section 3.3, stdio
and Streamable HTTP). Accept: one success and one error per tool M01-M69, the listings, `initialize`,
unknown tool, invalid arguments. Deps: CF-01.

**CF-04 HTTP extras**: `tools/contracts/record_http.py`, `contracts/http/*` (section 3.3). Accept:
X1-X7 recorded in each relevant state; 401, 404 (API and SPA), 405, 421, 422 bodies; headers O07; an
SSE sample with a domain event and a queue snapshot. Deps: CF-01.

**CF-05 Schema per revision and fixture databases**: `contracts/schema-0001.sql` to
`schema-0011.sql`, `golden/db/rev-000N.db` (populated with the demo seed at `0011`, and at each
earlier revision with rows in every table that exists there), `tools/contracts/dbdump.py` (canonical
dump: section 3.1 item 4). Deps: CF-01.

**CF-06 Event vocabulary**: `contracts/events.json`: every `event_type` with its entity type and
the keys of `data`, from a code scan and from the CF-05 databases. Deps: CF-01.

**LIB-01 wg-types**: ids, keys, `Timestamp` with the four formatters, the 23 enums with string
forms, `Actor`, `Permission` and the default sets, `WgError`, code to exit-code (O05) and to HTTP
status mappings. Accept: GV-12 timestamp vectors and GV-01 enum names pass; `Core` tier clean.
Deps: FD-01, GV-12.

**LIB-02 wg-json**: the O01, O06 and O10 dialects as `serde_json` formatters, Python float repr.
Accept: GV-12 vectors pass. Deps: FD-01, GV-12.

**LIB-03 wg-pyrand**: Accept: GV-06 vectors pass; `Pure` tier clean. Deps: FD-01, GV-06.

**LIB-06 wg-validate** (`Tier: mid`): Pydantic-shaped validation errors for the field types in GV-14.
Accept: GV-14 vectors pass. Deps: FD-01, GV-14.

**LIB-07 wg-render** (`Tier: mid`): tables, `kv`, markup handling, colour off when not a terminal,
width from `COLUMNS` or the terminal. Accept: GV-13 vectors pass. Deps: FD-01, GV-13.

**LIB-08 wg-config paths**: platformdirs rules and `WORKGRAPH_HOME`, `AppPaths` with every path of
F01-F11. Accept: GV-18 vectors pass. Deps: FD-01, GV-18.

**CR-01 Conformance runner skeleton** (`Tier: mid`)
Touches: `crates/wg-conformance`, `tests/conformance/scenario.schema.json`.
Accept: loads and validates scenario files; runs `cli` steps against an implementation (executable,
env, fresh home and temp dir), resolves placeholders, executes `assert` steps, writes a step log;
modes `diff`, `record`, `check` with a report of equal/allowlisted/failed per file. Deps: FD-01.

**TH-02 to TH-07 Interface tests through the helpers** (one task per row; each replaces direct
`CliRunner`, `TestClient` or MCP command construction with the TH-01 helpers, with no change to what
the tests assert; the files still pass in `python` mode on the stack). Deps: TH-01.

| Id | Files |
| --- | --- |
| TH-02 | `test_cli.py`, `test_cli_groups.py`, `test_task_files.py` |
| TH-03 | `test_api.py`, `test_mcp.py` |
| TH-04 | `test_claude_hooks.py`, `test_ui_screenshot.py`, `test_tracker_cli.py` |
| TH-05 | `test_triage.py`, `test_rename.py`, `test_contract_check.py` |
| TH-06 | `test_trials.py`, `test_planner.py`, `test_predictor.py`, `test_replay_pool.py` |
| TH-07 | `test_agent_time_report.py`, `test_subagent_report.py` |

### 9.5 Model, configuration and runner

**FD-03 Rust checks in the repository gate**: `tools/check.py` gains the Rust steps (fmt, clippy,
`cargo purity`, and `cargo test --workspace` behind a `--tests` flag used only by stack
verification). Deps: FD-02.

**LIB-04 wg-model conventions and the task domain** (`Tier: top`)
Touches: `crates/wg-model`.
Accept: `Write`, `EventDraft`, `Effect`, effect-result inputs, `Stamp`, `Decision<R>` and the id
derivation of 4.1; entity values and `TaskSlice`/`RegistrySlice` for workspaces, projects,
repositories (with checkouts, remotes, refs, dependencies), tasks (with files, tags, links,
comments, dependencies, relations, projects, repositories, external items); typed payloads for the
`task.*`, `project.*`, `repository.*` and `workspace.*` events of CF-06; a module comment with the
conventions the later model tasks follow (naming, one module per table family, how a slice is
declared, how a write names its row); value assertions of 4.6. Deps: LIB-01, CF-06.

**LIB-09 wg-config model**: load, validate (unknown keys rejected with Pydantic-shaped messages),
migrate v1, merge workspace files, `config set` path and JSON parsing, the `tomli_w` writer.
Accept: GV-11 vectors pass; every key of contracts section 13.5. Deps: LIB-06, LIB-08, GV-11.

**CR-02 Normaliser and allowlist**: section 3.4; `tests/conformance/divergences.toml` with DV1-DV7.
Accept: unit tests on recorded outputs; two Python runs of one scenario compare equal. Deps: CR-01.

**CR-03 Git fixture operations**: `init`, `commit` (fixed author, committer and dates as
`tests/conftest.py` `GitRepo` does), `branch`, `checkout`, `worktree`, `write`, `merge`. Deps: CR-01.

**CR-04 MCP driver**: stdio JSON-RPC client for `mcp` steps, raw message capture, CF-03 replay
command. Deps: CR-01, CF-03.

**CR-05 HTTP driver**: starts `serve --no-worker` on a free port, waits for `/api/v1/health`, reads
the token, `http` steps, SSE capture with `max_events`, CF-04 replay command. Deps: CR-01, CF-04.

**CR-06 Database dump in Rust**: equals `tools/contracts/dbdump.py` output byte for byte on every
CF-05 database. Deps: CR-01, CF-05.

**CR-07 Coverage report**: per contract id, the scenarios and tests that cover it; lists uncovered
ids. Deps: CR-01, CF-01.

### 9.6 Model completion, store base, first core modules, scenarios

**LIB-10 wg-model: execution entities**: attempts, revision manifests, managed worktrees, resource
leases, change sets, stacks, stack repositories and items, jobs, job runs, resource claims, workers,
system cursors, artifacts; `RunSlice`, `RunState`, `StackSlice`, `StackFlow`, `QueueSlice`; event
payloads for `attempt.*`, `stack.*`, `worktree.*`. Accept: conventions of LIB-04; value assertions.
Deps: LIB-04.

**LIB-11 wg-model: knowledge entities**: commits and their file changes, symbols, analyses,
task links, equivalences, change surfaces, object gaps; verification configs, runs, results,
acceptances; classifications, gold labels, evaluations, embeddings; automation rules, runs,
notifications, reports, findings; sessions, bindings, subagent runs, internal tasks, hook and tool
events; experiments, trials, predictor fits; the remaining slices and event payloads. Deps: LIB-04.

**LIB-05 wg-wire skeleton** (`Tier: mid`): the error envelope, DTO conventions (field order, per-field
timestamp format, optional-field rules matching Pydantic's `model_dump`), the embedded OpenAPI and
MCP descriptors, and a test that every OpenAPI component schema has a DTO with the same field names
in the same order, for the areas listed as implemented (5.5). Deps: LIB-02, LIB-04,
CF-01, CF-03.

**SH-01 wg-store base** (`Tier: mid`): connection setup, transactions, row codecs, the event append
(respecting the immutability triggers), applying a `Decision`'s writes and events in one
transaction, reading `events` (by entity, by workspace after `seq`). Accept: a written event row
read back by Python's `EventRecord` equals the Python-written form (stack test). Deps: LIB-04.

**CORE-01 Task lifecycle**: `wg-tasks` lifecycle module. Accept: GV-01 vectors pass; the lifecycle
cases of `tests/unit/test_tasks_core.py` ported as Rust tests. Deps: LIB-04, GV-01.

**CORE-06 Resolution rules**: task, project, repository and workspace references by UUID, key, slug or
name, with the Python error messages. Accept: cases from `test_foundations.py` ported. Deps: LIB-04.

**SC-01 to SC-36 Scenarios** (each: scenario files under `tests/conformance/scenarios/<area>/`
covering every test function of the named Python files, with the original assertions as `assert`
steps and `covers` ids; cases not reachable through an interface go to `not_observable` with the GV
task that covers them; Python goldens recorded with `record` mode). Deps: CR-01 to CR-05 (CR-06 for
the database comparisons).

| Id | Source or purpose |
| --- | --- |
| SC-01 | `test_git.py` |
| SC-02 | `test_worktrees.py`, `test_worktree_discovery.py` |
| SC-03 | `test_runs.py` |
| SC-04 | `test_stacks.py`, `test_stack_bases.py` |
| SC-05 | `test_stack_integration.py`, tests 1-10 |
| SC-06 | `test_stack_integration.py`, tests 11-20 |
| SC-07 | `test_stack_integration.py`, tests 21-30 |
| SC-08 | `test_stack_integration.py`, tests 31-40 |
| SC-09 | `test_verification_tiers.py` (integration) |
| SC-10 | `test_experiments.py` |
| SC-11 | `test_replay.py` |
| SC-12 | `test_tracker_sync.py` |
| SC-13 | `test_tracker_ownership.py` |
| SC-14 | `test_tracker_keys.py`, `test_tracker_repositories.py`, `test_tracker_end_to_end.py` (Jira through `tests/fixtures/fake_tracker.py`) |
| SC-15 | `test_analytics.py` |
| SC-16 | `test_providers_and_services.py` |
| SC-17 | `test_llm_classification_search.py` (Ollama through `tests/fixtures/fake_ollama.py`) |
| SC-18 | `test_automation.py` |
| SC-19 | `tests/e2e/test_demo.py` (both tests) |
| SC-20 | `test_tasks_core.py` service-level cases |
| SC-21 | `test_foundations.py` service-level cases (init, migrations at start, config files) |
| SC-22 | `test_jobs.py` and `test_tracker_jobs.py` through `worker run --once`, `job` commands |
| SC-23 | claude hook ingestion sequences (session, subagent, transcript, spool and replay) beyond `test_claude_hooks.py` |
| SC-24 | backup, restore, export, import, events verify |
| SC-25 | coverage: `task` commands C145-C167, every option at least once, each error code the group can return |
| SC-26 | coverage: `workspace`, `project`, `repo`, `config`, `init`, `doctor` (C058-C062, C065, C099, C108-C123, C180-C185) |
| SC-27 | coverage: `attempt`, `agent`, `run`, `verify` (C001-C002, C016-C024, C125-C132, C172-C176) |
| SC-28 | coverage: `stack`, `git`, `worker`, `job` (C086-C097, C100-C106, C135-C144, C177-C179) |
| SC-29 | coverage: `analytics`, `search`, `classify` (C003-C015, C038-C046, C133) |
| SC-30 | coverage: `automation`, `claude`, `tracker`, `events`, `export`, `import`, `backup`, `restore` (C025-C037, C047-C057, C066-C067, C084-C085, C098, C124, C168-C170) |
| SC-31 | coverage: `experiment` (C068-C083) |
| SC-32 | coverage: MCP M01-M25 and the 7 resources |
| SC-33 | coverage: MCP M26-M65 |
| SC-34 | coverage: HTTP H001-H061 |
| SC-35 | coverage: HTTP H062-H124, X1-X7, SSE |
| SC-36 | coverage: security (B10): tokens, Host/Origin, LAN pairing and lockout, `--allow-remote`, permissions denied for agents on every write tool and route |

### 9.7 Wire, core and shell crates

**LIB-12 to LIB-19 Wire DTOs by area** (each: DTOs, conversions from model values, the OpenAPI
component-schema test of LIB-05 passing for the area, and the MCP output shapes of the area's tools
from CF-03). Deps: LIB-05, LIB-10, LIB-11.

| Id | Area | Contracts |
| --- | --- | --- |
| LIB-12 | workspaces, projects, repositories, worktrees | H003-H022, M01-M08 |
| LIB-13 | tasks (detail, list, graph, ready, history, comments, files, links) | H023-H061, M26-M46 |
| LIB-14 | stacks | H062-H070, M09-M16 |
| LIB-15 | attempts, runs, jobs, workers, scheduler | H071-H084, H122, M47-M54 |
| LIB-16 | verification, acceptance, git changes and diffs | H085-H090, M55-M56 |
| LIB-17 | automation, notifications, settings, agents, llm, claude status, meta, health, events | H001-H002, H091-H100, H117-H121, H123-H124, M64-M65 |
| LIB-18 | analytics, reports, search, classification | H101-H116, M57-M63 |
| LIB-19 | experiments, trials, predictor, contract check | M17-M25, M28 |

**CORE-02 to CORE-29 Domain crates** (each: the decisions and queries as pure functions in the named
crate, `Core` tier clean, the named vectors pass, the named Python unit tests ported as Rust tests,
and the scenarios that exercise the area pass once the matching interface exists).

| Id | Crate: scope | Vectors and tests | Deps | Tier |
| --- | --- | --- | --- | --- |
| CORE-02 | `wg-tasks`: dependency graph, satisfaction incl. stack siblings, propagation, promote-ready, rollups, graph view | GV-19 | CORE-01, LIB-10 | mid |
| CORE-03 | `wg-tasks`: create, update, bulk update, archive, files, tags, content hash, version checks, tracked-task field ownership | `test_tasks_core.py`, `test_tracker_ownership.py` rules | CORE-02 | haiku |
| CORE-04 | `wg-tasks`: claim, release, expiry, complete, result, progress, comments, relations, repository links | `test_tasks_core.py` | CORE-02 | haiku |
| CORE-05 | `wg-tasks`: triage-apply validation and plan, rename-term plan | `test_rename_term.py`, `test_triage.py` cases | CORE-03 | haiku |
| CORE-07 | `wg-registry`: workspaces, projects, allowed roots | `test_foundations.py` cases | CORE-06 | haiku |
| CORE-08 | `wg-registry`: repositories, identity fingerprint, checkout discovery decisions, health | GV-03 (URLs) | CORE-06 | haiku |
| CORE-09 | `wg-gitcore`: parsers, file classes, languages, modules | GV-03 | LIB-11 | haiku |
| CORE-10 | `wg-gitcore`: attribution, rewrites, cherry-picks, churn, final change surface | GV-04, `test_attribution.py` | CORE-09 | haiku |
| CORE-11 | `wg-exec`: attempt state machine, external attempt start and submit plans, escalation | GV-01 | LIB-10 | haiku |
| CORE-12 | `wg-exec`: run flow state machine (prepare, run, capture, attribute, verify, conclude, cancel) | `test_runs.py` via SC-03 | CORE-11, CORE-16 | mid |
| CORE-13 | `wg-exec`: manifests, worktree and branch planning, stack base selection, change sets, recovery plans | GV-21 (`builds_on_others`) | CORE-11 | haiku |
| CORE-14 | `wg-stacks`: membership, item states, readmit, eject on evidence | GV-21 | LIB-10, CORE-02 | haiku |
| CORE-15 | `wg-stacks`: integration queue order, merge plans, conflicts, settle detection | GV-21 | CORE-14 | mid |
| CORE-16 | `wg-verify`: configs, check selection by scope and tier, failure signatures, run outcome, acceptance and auto-accept | GV-02, `test_failure_signatures.py`, `test_verification_tiers.py` (unit) | LIB-11 | haiku |
| CORE-17 | `wg-stacks`: verification on the tip, regression baselines and base reuse, errored checks, bisection, ejection and rebuild | GV-21, GV-02, `test_regression.py` | CORE-15, CORE-16 | top |
| CORE-18 | `wg-stacks`: landing plan, compare-and-swap, deferral and rebase, release of unverified children, refusals, auto-accept | GV-21 | CORE-17 | mid |
| CORE-19 | `wg-jobs`: queue, limits, leases, backoff, dead letter, idempotency, worker liveness, maintenance leadership and plan, retention | GV-20, `test_jobs.py`, `test_tracker_jobs.py` | LIB-10 | haiku |
| CORE-20 | `wg-jobs` scheduler and `wg-automation`: rules, catalog, preview, depth and rate limits, cron | GV-10 | CORE-19, LIB-11 | haiku |
| CORE-21 | `wg-intel`: heuristic provider, taxonomy, prompts and response parsing, evaluation, search ranking, FTS quoting | GV-09 | LIB-11 | haiku |
| CORE-22 | `wg-telemetry`: hook normalisation, redaction, dedupe, correlation and binding, transcripts, timing | GV-07, `test_transcript.py`, `test_transcript_timing.py` | LIB-11 | haiku |
| CORE-23 | `wg-trackers`: Jira mapping, status mapping, sync decisions | GV-08, `test_tracker_mapper.py` | LIB-11, CORE-03 | haiku |
| CORE-24 | `wg-experiments`: statistics and predictor | GV-05, `test_experiment_stats.py` | LIB-11, LIB-03 | haiku |
| CORE-25 | `wg-experiments`: protocol validation, staircase, planner, trials, grading and classification | GV-05, `test_staircase.py` | CORE-24 | haiku |
| CORE-26 | `wg-experiments`: contract check, briefs, escape, replay selection, pool | GV-17, `test_contract.py`, `test_briefs.py`, `test_escape.py` | CORE-24 | haiku |
| CORE-27 | `wg-analytics`: fact tables from rows | GV-15 | LIB-11 | haiku |
| CORE-28 | `wg-analytics`: metric registry and query engine (37 metrics, dimensions, filters, periods), overview, compare, coupling, failures, history, estimate, retrospective | GV-15 | CORE-27 | mid |
| CORE-29 | `wg-analytics`: subagents, agent time, bottlenecks; `wg-events`: upcasters and the `events verify` projection | GV-15, CF-06 | CORE-27 | haiku |

**SH-02 to SH-23 Shell crates** (each: the crate as described in 4.3, `cargo purity --candidates`
over it does not exceed the ceiling, tests that exercise real SQLite, git or processes as the
Python tests did).

| Id | Crate: scope | Deps | Tier |
| --- | --- | --- | --- |
| SH-02 | `wg-store` migrations: runner on `alembic_version`, revision `0001` DDL | SH-01, CF-05 | mid |
| SH-03 | `wg-store` migrations `0002`-`0011` incl. the `0009` data migration; each CF-05 fixture migrates to a dump and `sqlite_master` equal to Python's | SH-02 | haiku |
| SH-04 | `wg-store`: tasks, files, tags, links, comments, dependencies, relations, FTS maintenance | SH-01, LIB-04 | haiku |
| SH-05 | `wg-store`: workspaces, projects, repositories, checkouts, remotes, refs, dependencies | SH-01, LIB-04 | haiku |
| SH-06 | `wg-store`: attempts, manifests, worktrees, leases, change sets, artifacts | SH-01, LIB-10 | haiku |
| SH-07 | `wg-store`: stacks, stack repositories, items | SH-01, LIB-10 | haiku |
| SH-08 | `wg-store`: jobs, runs, claims, workers, cursors (atomic claim SQL as in `jobs/queue.py`) | SH-01, LIB-10 | haiku |
| SH-09 | `wg-store`: commits, file changes, symbols, analyses, commit links, equivalences, surfaces, gaps | SH-01, LIB-11 | haiku |
| SH-10 | `wg-store`: verification configs, runs, results, acceptances | SH-01, LIB-11 | haiku |
| SH-11 | `wg-store`: classifications, gold labels, evaluations, embeddings, search queries (bm25, embedding scan) | SH-01, LIB-11 | haiku |
| SH-12 | `wg-store`: automation, notifications, reports, findings | SH-01, LIB-11 | haiku |
| SH-13 | `wg-store`: sessions, bindings, subagent runs, internal tasks, hook and tool events, retention compaction | SH-01, LIB-11 | haiku |
| SH-14 | `wg-store`: experiments, trials, predictor fits, external items; analytics source extraction (`TABLES`) | SH-01, LIB-11 | haiku |
| SH-15 | `wg-git`: runner and every operation; `install-hook` file handling (F13) | LIB-01, CORE-09 | haiku |
| SH-16 | `wg-proc`: spawn, minimal environment, timeouts, tree kill, bounded capture, log streaming | LIB-01 | mid |
| SH-17 | `wg-agents`: Claude Code stream-json adapter (P02) and command adapter (P04), availability; tests with `tests/fixtures/fake_claude.py` replaced by a Rust fake | SH-16, CORE-22 | haiku |
| SH-18 | `wg-net`: Ollama, OpenAI-compatible, Jira clients with retries; `test_jira_client.py` cases ported with a local fake server | LIB-09 | haiku |
| SH-19 | `wg-codeparse`: tree-sitter extraction for six languages; GV-16 vectors pass | FD-01, GV-16 | haiku |
| SH-20 | `wg-archive`: backup and restore (F10), JSONL export and import (F14), CSV/JSON/Parquet (F15, DV6) | SH-01, LIB-02 | haiku |
| SH-21 | `wg-browser`: browser discovery (E05, E10) and CDP screenshot | LIB-01 | haiku |
| SH-22 | `wg-demo-agent`: the scripted agent of `demo/scripted_agent.py` in every mode, and the demo checks | LIB-02 | haiku |
| SH-23 | `wg-runtime`: stamping, load-decide-apply, effect interpreter, flow drivers, JSON logging, the app context used by every interface | SH-01, LIB-09, LIB-10, LIB-11 | top |

### 9.8 Runtime services and interface skeletons

| Id | Task | Deps | Tier |
| --- | --- | --- | --- |
| SH-24 | `wg-runtime`: worker threads, job dispatch for the 17 kinds, maintenance leader (lease recovery, lost workers, claim expiry, automation dispatch, schedules, scheduler, spool replay, retention, tracker scheduling), `worker recover` | SH-23, CORE-19, SH-08 | mid |
| SH-25 | `wg-runtime`: hook ingestion and spool replay paths (P01, F09, O15) | SH-23, CORE-22, SH-13 | haiku |
| IF-01 | `wg-cli` skeleton: argv handling (`--json` stripping, `--home`, `-w`, `--version`, no-args help), clap tree test against `contracts/cli.json` for the groups listed as implemented (5.5), error and exit-code mapping, JSON and human output plumbing | LIB-05, LIB-07, SH-23 | mid |
| IF-30 | `wg-mcp` skeleton: stdio and Streamable HTTP via `rmcp`, descriptors from the snapshot, argument validation, error mapping, resources; CF-03 listings and `initialize` replay equal | LIB-05, LIB-06, SH-23, CF-03 | mid |
| IF-40 | `wg-server` skeleton: router, security middleware, error envelope and 422 bodies, OpenAPI and Swagger, SPA and static files, the `/mcp/` mount, route registration test against the OpenAPI snapshot for the groups listed as implemented (5.5) | LIB-05, LIB-06, SH-23, CF-04 | top |
| IF-50 | `claude hook` fast path in `wg-cli`: dispatched before clap, reads at most 8 MiB, always exits 0 with no output | SH-25 | haiku |

### 9.9 Interface groups

Each CLI task implements the listed commands in `crates/wg-cli/src/commands/<group>.rs`, makes the
clap tree test pass for them, and its acceptance is the listed contract ids plus the scenarios that
cover them (SC-25 to SC-31 and the area scenarios) passing in `diff` mode and the matching TH files
passing in Rust mode.

| Id | Commands | Contracts | Deps |
| --- | --- | --- | --- |
| IF-02 | `init`, `doctor`, `config` | C058-C062, C065, C099 | IF-01, LIB-09, SH-03 |
| IF-03 | `workspace`, `project` (incl. `rename-term`) | C108-C114, C180-C185 | IF-01, CORE-05, CORE-07, LIB-12, SH-05 |
| IF-04 | `repo` | C115-C123 | IF-01, CORE-08, LIB-12, SH-05, SH-15 |
| IF-05 | `task create, list, show, update, bulk-update, archive, history, graph, ready, promote-ready` | C146, C147, C152, C154, C155, C157, C159, C160, C165, C167 | IF-01, CORE-03, LIB-13, SH-04 |
| IF-06 | `task claim, release, complete, progress, result` | C148, C150, C158, C163, C164 | IF-01, CORE-04, CORE-16, LIB-13, SH-04, SH-10 |
| IF-24 | `task accept`, `task reject` (incl. `--manual`, `--promote-dependents`, stack landing and `landing: deferred`) and their routes H033-H035 | C145, C161, H033-H035 | IF-01, IF-40, CORE-18, LIB-13, SH-10 |
| IF-07 | `task comment, depend, relate, link-repo, contract-check, triage-apply` | C149, C151, C153, C156, C162, C166 | IF-01, CORE-04, CORE-05, CORE-26, LIB-13 |
| IF-08 | `stack` | C135-C144 | IF-01, CORE-18, LIB-14, SH-07, SH-24 |
| IF-09 | `attempt`, `agent` | C001-C002, C016-C024 | IF-01, CORE-12, CORE-13, LIB-15, SH-06, SH-17 |
| IF-10 | `run`, `verify` | C125-C132, C172-C176 | IF-01, CORE-12, CORE-16, LIB-15, LIB-16, SH-10, SH-24 |
| IF-11 | `worker`, `job` | C100-C106, C177-C179 | IF-01, SH-24, LIB-15 |
| IF-12 | `git` | C086-C097 | IF-01, CORE-10, CORE-13, LIB-16, SH-09, SH-15, SH-19 |
| IF-13 | `tracker` | C168-C170 | IF-01, CORE-23, SH-14, SH-18 |
| IF-14 | `classify`, `search` | C038-C046, C133 | IF-01, CORE-21, LIB-18, SH-11, SH-18 |
| IF-15 | `analytics metrics, query, overview, compare, coupling, failures, history` | C004, C005, C008-C012 | IF-01, CORE-28, LIB-18, SH-14 |
| IF-16 | `analytics subagents, time, bottlenecks, estimate, export, retrospective` | C003, C006, C007, C013-C015 | IF-01, CORE-28, CORE-29, LIB-18, SH-20 |
| IF-17 | `automation` | C025-C036 | IF-01, CORE-20, LIB-17, SH-12 |
| IF-18 | `claude status, replay, bind, unbind, sessions, mcp-config, backfill-time, backfill-transcripts` | C047-C049, C052-C056 | IF-01, CORE-22, SH-13, SH-25 |
| IF-19 | `experiment create, list, show, freeze, close, next, report, fit, predict` | C068-C074, C077, C078 | IF-01, CORE-25, LIB-19, SH-14 |
| IF-20 | `experiment trial add/show/start/grade/classify`, `experiment replay scan/create` | C075, C076, C079-C083 | IF-01, CORE-25, CORE-26, SH-15, SH-16 |
| IF-21 | `backup`, `restore`, `export`, `import`, `events` | C037, C066, C067, C084, C085, C098, C124 | IF-01, CORE-29, SH-20 |
| IF-22 | `demo e2e`, `demo seed` (DV4) | C063, C064 | IF-01, SH-22, SH-24 |
| IF-23 | `ui screenshot`, `serve`, `mcp` command wiring | C107, C134, C171 | IF-01, SH-21, IF-30, IF-40 |
| IF-51 | `claude install`, `claude uninstall` (F12, scopes, backups, idempotence) | C051, C057 | IF-01 |

MCP tool groups (each: the tools return the CF-03 recorded results in `diff` mode and SC-32/SC-33
pass for them; permission checks as in section 10 of the inventory):

| Id | Tools | Deps |
| --- | --- | --- |
| IF-31 | M01-M08 workspace, project, repository | IF-30, CORE-07, CORE-08, LIB-12 |
| IF-32 | M26-M46 task tools | IF-30, CORE-03, CORE-04, CORE-26, LIB-13 |
| IF-33 | M47-M56 attempt, run, git tools | IF-30, CORE-12, CORE-13, LIB-15, LIB-16 |
| IF-34 | M09-M16 stack tools | IF-30, CORE-18, LIB-14, SH-24 |
| IF-35 | M17-M25 experiment and trial tools | IF-30, CORE-25, CORE-26, LIB-19 |
| IF-36 | M57-M65 classification, analytics, automation tools; the 7 resources | IF-30, CORE-20, CORE-21, CORE-28, CORE-29, LIB-17, LIB-18 |

HTTP route groups (each: the routes registered and in the OpenAPI snapshot, SC-34/SC-35 and the
`test_api.py` cases for them pass):

| Id | Routes | Deps |
| --- | --- | --- |
| IF-41 | LAN listener, pairing (X4, X5), device cookie, lockout; SC-36 | IF-40 |
| IF-42 | H001-H005, H117-H121 meta, settings, workspaces, agents, llm, claude status | IF-40, LIB-12, LIB-17, LIB-09 |
| IF-43 | H006-H022 projects, repositories, worktrees | IF-40, CORE-07, CORE-08, LIB-12 |
| IF-44 | H023-H046 task reads, writes and lifecycle, except H033-H035 (IF-24) | IF-40, CORE-03, CORE-04, LIB-13 |
| IF-45 | H047-H061 task commits, changes, diff, classification, estimate, verification, decompose, schedulability | IF-40, CORE-10, CORE-16, CORE-21, CORE-28, LIB-13, LIB-16 |
| IF-46 | H071-H084, H122 attempts, runs, jobs, workers, scheduler | IF-40, CORE-12, CORE-19, LIB-15 |
| IF-53 | H062-H070 stacks | IF-40, CORE-18, LIB-14, SH-24 |
| IF-47 | H085-H100 verification, automation, notifications | IF-40, CORE-16, CORE-20, LIB-16, LIB-17 |
| IF-48 | H101-H116 analytics, reports, search, classification | IF-40, CORE-21, CORE-28, CORE-29, LIB-18 |
| IF-49 | H123-H124 events and the SSE stream (O08) | IF-40, SH-24, LIB-17 |

### 9.10 Parity

| Id | Task | Deps | Tier |
| --- | --- | --- | --- |
| VF-01 | Python interface suite in Rust mode (`WORKGRAPH_IMPL=rust`): every TH file passes; failures are bisected to the owning IF task and sent back with the output | all IF tasks, TH-02 to TH-07 | haiku |
| VF-02 | Full corpus in `diff` mode: zero failed steps, every allowlist entry names a decision; CR-07 shows every C, M, H and X id covered or listed with a reason | all IF and SC tasks | mid |
| VF-03 | Data compatibility (L5): every CF-05 database migrated by Rust equals Python's; the Python-writes/Rust-reads and Rust-writes/Python-reads replays; `doctor --verify-events` clean in both directions | VF-02 | haiku |
| VF-04 | Mixed queue: one Python and one Rust worker drain the demo seed's jobs and a stack scenario concurrently; final dump equals a single-implementation run | VF-02, SH-24 | haiku |
| VF-05 | Measured comparison: startup of `--version`, `task show --json`, `claude hook`; `task list` on the demo seed and on a 5,000-task generated database; MCP `tools/list`; a stack verification; numbers recorded in the task, Rust not slower on any | VF-02 | haiku |

### 9.11 Switch and retirement

| Id | Task | Deps |
| --- | --- | --- |
| CO-01 | Side-by-side install as `workgraph-rs` (5.3) and a `doctor` check that both binaries see the same home and schema | VF-01 to VF-05 |
| CO-02 | Rust goldens: record the corpus with Python into `tests/conformance/goldens/`, switch the stack verification to `check` mode so it no longer needs Python | VF-02 |
| CO-03 | Switch the installed `workgraph` to the Rust binary (5.3), verify the MCP registration and the hook handlers, document the rollback in `docs/getting-started.md` | CO-01, user's decision Q12 |
| CO-04 | Retire Python (5.4) | CO-02, CO-03, user's decision Q12 |
| CO-05 | Documentation rewrite: `README.md`, `architecture.md`, `testing.md`, `getting-started.md`, `extending.md`, `troubleshooting.md` describe the Rust implementation; this document's status updated | CO-04 |

### 9.12 Feature crates (D15)

These come before the remaining interface groups of 9.9: every group not yet ported depends on
FC-03 and is written directly in the feature-crate shape, inside its feature's crate, so nothing
new lands in the per-layer crates. A move task carries the feature's modules out of every layer
crate, deletes them there, and changes no behaviour. Its acceptance is the existing tests of that
feature passing unchanged (moved, not rewritten) plus the gates.

| Id | Task | Deps | Tier |
| --- | --- | --- | --- |
| FC-01 | `Feature` descriptor in `wg-runtime` (CLI groups, routes, tools, resources, job handlers as `fn` tables); `wg-cli`, `wg-server`, `wg-mcp` and the worker collect from `&[&Feature]` beside today's tables; the `IMPLEMENTED` lists of the clap-tree, route and tool coverage tests derived from the list | none | mid |
| FC-02 | arch-lint: `crate::module Tier` lines in `purity-tiers.txt`, unlisted modules of a listed feature crate fail, `Core` modules checked on `use` items and paths, shell ceiling measured over `Shell` modules | none | mid |
| FC-03 | Pilot: `wg-decisions` from the decision code in `wg-runtimespec`, `wg-runtime`, `wg-storecodec`, `wg-store`, `wg-clispec`, `wg-serverspec`, `wg-server`, `wg-mcp` and `wg-wire`; Python `test_decisions.py` in Rust mode and the HTTP replay unchanged; 4.16 amended with anything the pilot changes | FC-01, FC-02 | mid |
| FC-04 | Move `tasks` (including `acceptance.rs` task acceptance) into `wg-tasks-feature` (pure shared parts into `wg-tasks`) | FC-03 | mid |
| FC-05 | Move `registry`, `repos`, `settings`, `setup` into `wg-registry-feature` (pure shared parts stay in `wg-registry`) | FC-03 | haiku |
| FC-06 | Move `git` and `git_sync` into `wg-git-feature` (`wg-gitcore` stays the pure core) | FC-05 | haiku |
| FC-07 | Move `worktrees` and the attempt/run modules into `wg-exec-feature` | FC-04, FC-05 | haiku |
| FC-08 | Move `stacks` and stack landing into `wg-stacks-feature`, and reconcile WG-3772's executor with the landing in `acceptance.rs` into one implementation | FC-04, FC-07 | top |
| FC-09 | Move job handlers out of `wg-runtime` into their features' `jobs` modules; `HandlerRegistry::standard()` assembled from `FEATURES` | FC-03 | mid |
| FC-10 | Move `automation` into `wg-automation-feature` | FC-09 | haiku |
| FC-11 | Move the remaining per-area store modules (experiments, analytics, classify, search, sessions and hooks, external/trackers, verification) into their `wg-<area>-feature` crates | FC-03 | haiku |
| FC-12 | Retire the per-layer crates: whatever is left of `wg-runtimespec`, `wg-storecodec`, `wg-clispec`, `wg-serverspec`, `wg-mcpspec` is base code and moves into the base crates or stays as a renamed base crate; 4.3's table rewritten to the final layout | FC-04 to FC-11 | mid |

## 11. Decisions from the user (2026-10-25)

The user adopted the recommended answers to the open questions:

1. `--help` and usage errors keep the same content; clap's layout is acceptable (help text is not parsed by tools).
2. Human output is UTF-8 everywhere, including when piped on Windows.
3. Python contracts are frozen during the rewrite. Defect fixes are allowed, each with a paired Rust task, so both versions change together.
4. Schema changes are planned in both implementations and land in one stack, not frozen: `0011` (task decisions) is the first (D9).
5. `arch-lint` is copied into this repository, not shared with noether.
6. `/api/docs` is served byte-identical, CDN links included.
7. The one-line Vite output-directory change is acceptable at retirement.
8. The demo uses a companion `workgraph-demo-agent` instead of `python check.py`.
9. Statistics computed through `exp`/`log` are compared with a 1e-12 relative tolerance.
10. The coordinator adds the Rust checks to this repository's verification config (the CLI runs as the user, who holds `config:manage`).
11. At retirement the 164 interface tests that drive the CLI, MCP and HTTP stay as contract tests against the Rust binary; the 502 in-process tests are deleted with the Python code.
12. The side-by-side period ends after one complete real stack-landing cycle on the Rust binary with zero divergence reports.

Work lands on the `rust-integration` branch through tested stacks and is merged into `main` (the live checkout) only at green waves.

## 12. Decisions from the user (2026-10-26)

1. Restructure into vertical feature crates (D15, 4.16, tasks FC-01 to FC-12) before porting the
   remaining interface groups. A feature owns its slice from SQL to CLI, HTTP and MCP; purity tiers
   are assigned per module, so the per-layer pure/shell crate pairs are retired.
2. Conformance normalisation (WG-3758 decision, option a): set-like id lists (`depends_on`,
   `blocks`, `child_ids`, `project_ids`, `repository_ids`) are sorted by the normaliser, and
   timestamps written within one command less than ~900 ms apart rank as one instant (fixture
   dates 1 s apart keep their order). Ordered lists such as `task graph` edges are still compared
   in order. DV1 is scoped to help and usage-error text, and DV3 to the DuckDB file, so neither can
   hide other differences.
3. DV8 and DV9 confirmed; DV10 (LF line endings) added.
4. `config show` colour matches Python: SGR styling with rich's JSON theme only when stdout is a
   terminal or `FORCE_COLOR` is set; `wg-render` gains a styling layer for it.
