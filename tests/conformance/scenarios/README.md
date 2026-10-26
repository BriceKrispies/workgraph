# Conformance scenarios

Scenario files are the differential corpus of the Rust rewrite (docs/rust-rewrite.md, section 3.2 L3
and 3.5). Each file is one JSON document that the `wg-conformance` runner executes against an
implementation in a fresh `WORKGRAPH_HOME` and a fresh temporary directory. The format is defined by
`tests/conformance/scenario.schema.json`; `crates/wg-conformance/src/scenario.rs` validates it.

## Layout and naming

* One directory per area under `tests/conformance/scenarios/` (`git/` first; SC-02 onward add more).
* One file per scenario, named `<id>.json`, where `id` equals the file stem and matches
  `^[a-z0-9][a-z0-9-]*$`. Prefix ids with the area, for example `git-merge-commits`.
* Cover one Python test function per scenario where the function's steps fit one command sequence.
  Split a function into several scenarios only when its sub-cases need separate fresh state.
* `covers` lists only the contract ids (C, M, H, X and so on) that the scenario's own steps exercise.
  Do not list ids a step does not reach. The coverage report (CR-07) relies on this.

## Steps

* `setup` runs before every step, in order. It is the only way to create commits and files:
  `git init`, `commit` (with `files` and `message`), `branch`, `checkout` (`-b` allowed), `merge`
  and `worktree`. There is no `rm`, `mv`, `rebase`, `cherry-pick` or `reset`, and no step creates
  commits, so every commit a scenario needs must be in `setup`, in the order it is made.
* Task keys are allocated from the workspace counter, starting at `WG-1` in a fresh home. A setup
  commit message may therefore name a key that a later `task create` step will produce (for example
  `Task-ID: WG-1`), as long as the tasks are created in the same order. Keys are not normalised.
* `cli` steps set `json: true` in almost every case, so the Python human renderer (which depends on
  width) is not part of the observed output. Each `json` assertion reads the most recent `cli` or `http` output (see the `http` note below).
* Use `save` with a JSON pointer (`"/items/0/sha"`) to carry a value into later steps as
  `{var:name}`. Placeholders `{repo:<name>}`, `{key:N}` and `{id:N}` resolve from earlier outputs.
* `assert` steps carry the original Python test's assertions, so a scenario is meaningful without a
  differential run. `json` equality values may use placeholders. Database assertions (`db`) are
  skipped by the runner skeleton and must not be relied on yet.
* Assertions on a file path are relative to `{home}` when not absolute. Use `{repo:<name>}` for
  repository files.
* `http` steps take `headers` (an object of header name to value) and `expect_status` (checked
  like `expect_exit`). Unless the step names its own `authorization` header, the API bearer token
  is sent; `"authorization": ""` sends an empty header, which is how a request without a token is
  made. A Host or Origin header can be given the same way. An `http` step becomes the last output:
  its exit is the HTTP status and its stdout the body (or the events with `max_events`), so `json`
  and `stdout_contains` assertions read the body and `{last:...}` reads it too.
* `{var:port}` is the port of the step's `serve` and `{var:token}` its API token; they are set by
  the first `http` step. Use them for an Origin that names the server, or a `?token=` query.

## Not observable

`not_observable` lists each part of a Python test function that cannot be reached through an interface
with the current runner. Each entry has:

* `case`: the test path (`tests/.../test_x.py::test_y`), then what is not covered and why, in one
  sentence. The reason lives here because the schema has no separate reason field.
* `covered_by`: the task that covers the behaviour instead (a golden-vector task such as `GV-03`, a
  core or shell crate task such as `CORE-10`, or a scenario task such as `SC-14`).

Never drop a case silently. If a sub-case can be expressed, assert it; the rest goes to
`not_observable`.

## Recording goldens

* `wg-conformance record --scenarios <dir> --python "uv run --project <worktree> workgraph" --env COLUMNS=`
  runs the Python implementation and writes `tests/conformance/golden/<id>.json`.
* `record` writes the golden before it judges the run, so a scenario whose assertions fail still
  leaves a golden. Read the report, and fix the scenario or the Python behaviour, before committing.
* `--env COLUMNS=` is a temporary workaround on Windows. With `COLUMNS=120` (the parity environment,
  docs/rust-rewrite.md section 3.1) the Python CLI fails at import in `cli/common.py` (`CliConsole`
  reads `self.file` before `_file` is set, when `legacy_windows` is evaluated). An empty value
  avoids it. Only width-dependent human output differs, and scenarios here use `--json`, so the
  goldens are unaffected. Remove the workaround once the Python console is fixed or the goldens are
  recorded on a POSIX machine.
* `wg-conformance check` against a Rust binary compares with the goldens; it is the cut-over gate.

## Runner gaps

Gaps that block cases are listed in `not_observable`. Open runner gaps, for the owning tasks:

* setup has no file deletion, move, rebase, cherry-pick or reset, and steps cannot create commits
  between syncs (affects git scenarios that need history rewritten or commits made mid-run);
* `load_scenarios` reads one directory level only, so `--scenarios tests/conformance/scenarios`
  does not load the area subdirectories; pass the area directory until the loader recurses;
* `db` assertions are skipped by `runner.rs` until the database driver lands.
