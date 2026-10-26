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

* `setup` runs before every step, in order. It names a typed git operation on a repository
  (`{"git": "commit", "repo": "api", ...}`). The operations are `init`, `commit`, `write`, `branch`,
  `checkout` (`-b` allowed), `worktree`, `merge`, `delete`, `move`, `rebase`, `cherry-pick` and
  `reset`, with the argument shapes that `check_git_op` accepts.
* A `git` step runs the same operations between steps, in the directory `cwd` instead of a repository
  name: `{"git": "commit", "cwd": "{var:checkout}", "files": {...}, "message": "..."}`. Use
  `{repo:<name>}` for a repository, or a saved path for a managed checkout. A git step does not
  change the most recent `cli` output. A failed git step fails that step only.
* An `rmtree` step removes a directory tree, for example a managed checkout removed out of band:
  `{"rmtree": "{var:checkout}"}`. The path must lie inside the sandbox home and must not be the home
  itself, and a `..` component is refused.
* Task keys are allocated from the workspace counter, starting at `WG-1` in a fresh home. A setup
  commit message may therefore name a key that a later `task create` step will produce (for example
  `Task-ID: WG-1`), as long as the tasks are created in the same order. Keys are not normalised.
* `cli` steps set `json: true` in almost every case, so the Python human renderer (which depends on
  width) is not part of the observed output. Each `json` assertion reads the most recent `cli` or `http` output (see the `http` note below).
* Use `save` with a JSON pointer (`"/items/0/sha"`) to carry a value into later steps as
  `{var:name}`. Placeholders `{repo:<name>}`, `{key:N}` and `{id:N}` resolve from earlier outputs.
* `assert` steps carry the original Python test's assertions, so a scenario is meaningful without a
  differential run. A `json` assertion takes a JSON pointer and exactly one form: `equals`,
  `not_equals` (the value exists and differs), `absent: true` (the key is not present) or `excludes`
  (the array at the pointer has no element equal to the value). Values may use placeholders. Use
  `excludes` for "the list has no such element". Database assertions (`db`) are skipped by the runner
  skeleton and must not be relied on yet.
* Assertions on a file path are relative to `{home}` when not absolute. Use `{repo:<name>}` for
  repository files.
* `{sep}` is the platform path separator (`\` on Windows, `/` elsewhere). Build paths with it
  (`{home}{sep}data`) rather than writing a separator, so a path assertion holds on every OS.
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

* `wg-conformance record --scenarios <dir> --python "uv run --project <worktree> workgraph"`
  runs the Python implementation and writes `tests/conformance/golden/<id>.json`.
* `record` writes the golden before it judges the run, so a scenario whose assertions fail still
  leaves a golden. Read the report, and fix the scenario or the Python behaviour, before committing.
* The runner sets `COLUMNS=120` and `NO_COLOR=1` (the parity environment, docs/rust-rewrite.md section
  3.1) for every run. The Python console fix on `rust-integration` (`CliConsole` no longer reads
  `self.file` before `_file` is set) makes that work on Windows, so no `--env` override is needed.
* `wg-conformance check` against a Rust binary compares with the goldens; it is the cut-over gate.

## Runner gaps

Gaps that block cases are listed in `not_observable`. Open runner gaps, for the owning tasks:

* `rmtree` removes only trees inside the sandbox home, so a linked worktree created beside the
  home (`../wt/...` from a repository) cannot be removed by a step, and `git worktree remove` has
  no step operation;
* `worktree lock` and `worktree remove` have no setup or step operation;
* `db` assertions are skipped by `runner.rs` until the database driver lands;
* the change-set item of a submitted attempt (its `head_sha`) and the database-only fields of
  rewrite equivalences have no CLI output to assert on.
