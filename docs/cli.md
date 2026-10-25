# CLI

`workgraph --help` lists every group; each command has `--help`. Global options:

| Option | Effect |
| --- | --- |
| `--json` (anywhere) | Machine-readable JSON on stdout; errors as `{"error": {"code","message","details"}}` |
| `--home DIR` | Use an isolated Workgraph home (same as `WORKGRAPH_HOME`) |
| `-w/--workspace SLUG` | Default workspace for the invocation (same as `WORKGRAPH_WORKSPACE`) |

Exit codes: `0` success, `1` unexpected error, `2` validation, `3` conflict (state, version,
busy resource), `4` not found, `5` permission denied, `6` provider unavailable, `7` configuration.

## Command groups

| Group | Commands |
| --- | --- |
| `init`, `doctor` | create storage/schema/first workspace; health checks (`--verify-events`) |
| `serve` | HTTP API + UI + Streamable HTTP MCP at `/mcp/` + in-process worker (`--no-worker`, `--no-mcp`); also serves paired devices on the local network unless `--local` |
| `mcp` | MCP server over stdio (default) or HTTP; `--agent-name`, `--grant run:start` |
| `workspace` | `create`, `list`, `show`, `use`, `archive`, `set --allowed-root` |
| `project` | `create`, `list`, `show`, `update` (`--name`, `--description`, `--status`), `add-repo`, `remove-repo`, `rename-term PROJECT --from OLD --to NEW [--keep TOKEN]... [--dry-run]` (renames a word, such as a language's name, across the project's live records: open tasks' title, description, acceptance criteria and file paths; every task's tags; the project's slug, name and description; its repositories' names and descriptions; and every string in the checks of verification configs scoped to them. Accepted, cancelled and archived tasks keep their text as records and take only the renamed tags. The word matches in three spellings (`rho`, `Rho`, `RHO`) when no letter precedes it and no letter of its case follows, so `rhombus` stays; `--keep` protects tokens that contain it but are not it. Each change is an ordinary event with reason `rename OLD to NEW` and queues no reclassification. A rewritten absolute path in a check that names nothing on disk is reported; checkout paths are not moved) |
| `repo` | `register`, `list`, `show`, `refresh`, `status`, `update`, `remove`, `remove-checkout`, `depend` |
| `task` | `create`, `list`, `ready`, `promote-ready [--tag T] [--project P] [--dry-run]` (moves every backlog task whose dependencies are all accepted to `ready` in one transaction and prints the promoted keys; skips tasks with children and tasks tagged `parked`), `show`, `contract-check TASK --model M [--target 0.8]` (triage: `not_delegable`, `incomplete`, `split` or `leaf`, with the missing contract fields, reasons and the predicted success, labelled an inference; `--json` for the verdict), `update`, `bulk-update`, `claim`, `release`, `complete`, `accept`, `reject`, `comment`, `progress`, `result`, `depend`, `relate`, `link-repo`, `archive`, `history`, `graph` (`--project`, `--repo`, `--open`), `triage-apply FILE [--dry-run] [--source TEXT]` (applies a JSON array of triage verdicts; see [Triage verdicts](#triage-verdicts)), `import-breakdown FILE --section TEXT --parent KEY [--tag T]... [--project P] [--repo R] [--dry-run]` (creates or updates the tasks of one markdown section; see [Importing a breakdown](#importing-a-breakdown)) |
| `decision` | `list [--status open\|decided\|withdrawn\|all]`, `show ID`, `request TASK --question Q [--option "label::detail"]... [--recommend N]` (options are lettered a, b, ... in order; `--recommend` is the 1-based position of the recommended one; no `--option` asks for free text; tags the task `needs-decision`), `choose ID (--option OPTION_ID \| --other TEXT) [--note TEXT]` (answers or changes the answer; the last open decision on a task clears `needs-decision` and `parked`), `withdraw ID [--reason TEXT]`. `ID` may be the 8-character prefix `list` prints. See [Decisions](api.md#decisions) |
| `experiment` | `create SLUG --title T [--question Q] --protocol @file.json\|JSON`, `list [--status]`, `show EXP` (protocol summary and trials in order), `freeze EXP` (validates the protocol, freezes it and starts the experiment), `close EXP`, `trial add EXP TASK --arm A --phase P [--band N]` (a planned trial of a running experiment), `trial show TRIAL`, `trial start TRIAL` (clones the task's replay base into a fresh single-branch clone with no remote, runs the protocol's `setup`, prints the worktree, branch, base sha and brief), `trial grade TRIAL [--timeout S]` (runs the oracle and gates in the clone and records the outcome; a failure with flags is classed `gaming`), `trial classify TRIAL --cause C --source human\|agent\|mechanical [--notes N]`, `next EXP` (the pending trial, or a newly planned one from the protocol, or `done` with the reason), `report EXP` (per arm: bands with Wilson intervals, size fit with bootstrap intervals, usage, failure causes, invalid trials; `--json` for the data), `fit --model M [--repo-penalty X] [--feature-penalty X] [--samples N] [--seed S]` (fits the success predictor for one model over every graded trial with ex-ante features, stores the fit with its data cutoff and trial ids, and bootstraps it), `predict --model M (--task TASK \| --repo R --lines N --files F --packages P,Q) [--fit ID]` (probability of success with a 95% interval and the fit it came from; labelled an inference; `--packages` is a comma-separated list, may be empty) (see [Experiments](data-model.md#experiments)) ; `replay scan --repo R [--rev REV] [--limit N] [--min-lines N] [--max-lines N] [--include-excluded]` (commits that could become replay tasks, with size band and exclusion reasons), `replay create --repo R SHA... [--validate] [--oracle-template JSON-list] [--env K=V]... [--timeout S]` (one replay task per commit: `created`, `exists`, `skipped` or `rejected`) |
| `stack` | `create PARENT`, `show PARENT` (repositories with base, integration branch and tip; children in merge order), `list [--all]`, `add PARENT TASK` and `remove PARENT TASK` (change the children of the active stack), `readmit PARENT TASK` (queue an ejected or pending child for integration again without re-running its fast gate, for an ejection caused outside the change), `eject PARENT TASK -m REASON` (eject a merged child the evidence already names as the failure and rebuild without it, skipping bisection), `history PARENT` (the stack's events in order, with their verification runs), `integrate PARENT` (queues merging the ready children onto each repository's integration branch), `verify PARENT` (queues full verification of the integration tip, bisecting and ejecting a failing child). Acceptance goes through `task`: `task accept PARENT` (or `--manual --reason`) lands a verified stack on the default branch, refused with `landing: deferred` when the default branch moved and the stack is rebuilt to verify again; `task accept CHILD` accepts one child alone and removes it from its stack; `task reject PARENT` leaves the stack as it is (see [Git](git.md#stack-landing)) |
| `attempt` | `list`, `show`, `create`, `start TASK [--agent-name N] [--adapter A] [--model M] [--isolation worktree\|in_place]`, `submit ATTEMPT [--failed] [--summary TEXT] [--error TEXT]`, `abandon ATTEMPT [--reason TEXT]` (an external attempt whose agent stopped without submitting: marked abandoned, the task returns to ready and its worktrees are released), `finish`, `compare`, `manifests` |
| `agent` | `list`, `add-command` |
| `run` | `start [--wait]`, `get`, `watch`, `cancel`, `retry`, `rerun`, `schedule [--dry-run/--explain]`, `decompose [--create]` |
| `verify` | `config`, `configs`, `run [--tier fast\|full]`, `runs`, `failures` |
| `worker` | `run [--once/--kind]`, `list`, `recover` |
| `job` | `list`, `show`, `stats`, `cancel`, `retry`, `enqueue`, `compact` |
| `git` | `sync [--full]`, `log`, `show`, `link`, `unlink`, `changes`, `diff`, `surface`, `analyze`, `worktrees`, `cleanup-worktrees [--apply]`, `install-hook` |
| `tracker` | `list` (configured connections), `sync NAME [--full] [--queue]` (mirrors a Jira connection's issues into tasks now and prints the report; `--queue` enqueues a `tracker_sync` job and prints its id), `status [NAME]` (cursor, last sync, tracked task counts, unresolved pull request repositories; all connections when no name). Read-only towards Jira; see [Configuration](configuration.md#issue-trackers) |
| `classify` | `request [--now]`, `show`, `confirm`, `promote`, `reclassify`, `gold`, `evaluate`, `evaluations`, `disagreements` |
| `search` | full-text / semantic / hybrid; `--similar-to KEY`; `--reindex` |
| `analytics` | `metrics`, `query`, `overview`, `compare`, `coupling`, `failures`, `subagents`, `time`, `bottlenecks`, `estimate`, `history`, `export`, `retrospective` |
| `automation` | `list`, `create`, `update`, `show`, `enable`, `disable`, `trigger`, `preview`, `runs`, `notifications`, `catalog`, `process` |
| `claude` | `install [--mcp]`, `uninstall`, `status`, `replay`, `bind`, `unbind`, `sessions`, `mcp-config`, `hook` |
| `config` | `show`, `path`, `validate`, `migrate`, `set` |
| `ui` | `screenshot [PAGE]`: capture a page of the running web UI to a PNG (see [below](#ui-screenshots)) |
| `backup`, `restore` | consistent online backup zip; verified restore with safety copy and forward migration |
| `export`, `import` | `export workspace FILE.jsonl`, `export analytics`; `import FILE.jsonl` |
| `events` | `list`, `verify` |
| `demo` | `e2e`, `seed` |

Worktree discovery: `repo register` and `repo refresh` run `git worktree list` and record each linked worktree (kind `worktree`) and the main working tree (kind `clone`) as a checkout of the same repository. Workgraph's managed worktrees, bare entries and paths outside the workspace's allowed roots are not added; the skipped paths are listed in the repository's `health_detail.skipped_worktrees`. Refresh marks worktrees that were removed or are reported as prunable as unavailable, and records `repository.checkout_discovered` / `repository.checkout_lost` events.

`repo update` takes `--track BRANCH` and `--untrack BRANCH` (both repeatable) to edit the repository's
tracked branches. Git sync scans the tracked branches first and then only the first 20 local branches,
so a branch that matters in a repository with many branches should be tracked explicitly. Changing
`--default-branch` also adds the new default to the tracked branches when it is not tracked yet; existing
entries are kept.

## Promoting ready work

`task promote-ready` finds the backlog tasks whose dependencies are all accepted and moves them to `ready`,
without one `task show` per task. A scope is required: `--tag` (repeatable), `--project`, or `--all` for every
backlog task in the workspace (`--all` cannot be combined with the others). The result lists the keys that moved and
a count per project; `--dry-run` reports the same and changes nothing. A task with no dependencies counts as having
them all accepted.
Tasks with children are skipped (they are rolled up, not claimed), as are tasks tagged `parked` and archived
tasks. Dependencies count as accepted under the same rule as `task ready` (including a sibling already
integration-ready in an active stack).

`task accept TASK --promote-dependents` promotes the direct backlog dependents of TASK that are fully satisfied
after the acceptance, with the same skip rules. Its JSON output is the acceptance with a `promoted` list added.

## Importing a breakdown

`task import-breakdown FILE.md --section "HEADING" --parent KEY` turns one section of a planning document
into tasks. The section runs from its heading to the next heading of the same or a higher level.

- A task is a bold line at the start of a line: `**V12 Title**`, `**V1. Title**`, `**FD-01 Title**`. The text
  under it, up to the next task header, is the description.
- A ranged header, `**GV-01 to GV-21 Name**`, is followed by a markdown table whose first column holds one id per
  row. Each row becomes a task titled `Name: <second column>`; its description is the block text plus the row.
  A table may also have `Deps` and `Tier` columns (as the interface tables of `docs/rust-rewrite.md` do); those
  apply to their row only.
- `Depends on:` or `Deps:` lists name other ids (`SC-01 to SC-36` expands) or existing task keys (`WG-12`).
  Anything else in such a list (`Q1 answered`) stays in the description and is reported as unresolved.
  A task with an unresolved item stays in `backlog`.
- `Tier: mid` and `Tier: top` add the tags `tier-mid` and `tier-top`. `--tag` adds a tag to every task, `--parent`
  sets the parent of every task, and `--project` and `--repo` apply to the tasks this run creates.
- A task whose dependencies are all accepted (or that has none) is `ready`; the rest are `backlog`.
- Each task stores `metadata.breakdown_id` (its id) and `metadata.breakdown_doc` (the file's absolute path, a `#`,
  then the section heading). A re-run finds those tasks and updates their title, description, tier and `--tag`
  tags, parent and new dependencies; unchanged tasks are left alone, and a `backlog` task whose dependencies are now
  accepted becomes `ready`. A re-run never creates a duplicate and never removes a dependency or changes the status
  of a task that has started (remove a stale dependency with `task depend --remove`).
- The whole section is validated before any write (a missing heading, a ranged block without a table, or a repeated
  id is refused). `--dry-run` prints the plan with `key` empty for tasks it would create and changes nothing.
- Output is a table of id, key, action (`created`, `updated`, `unchanged`), status and title, followed by the
  unresolved dependencies. With `--json` it is `{"tasks": [...], "map": {id: key}, "unresolved": [...], "counts": {...}, "dry_run", "source"}`.

The MCP tool `task_import_breakdown` takes the document text (`markdown`), `section`, a `source` name for the
metadata, `parent`, optional `tags`, `project`, `repository` and `dry_run`.

```sh
uv run workgraph task import-breakdown docs/rust-rewrite.md --section "9. Task breakdown" --parent WG-3362 --dry-run
uv run workgraph --json task import-breakdown docs/rust-rewrite.md --section "9. Task breakdown" --parent WG-3362
```

## Triage verdicts

`task triage-apply FILE` applies a JSON array of verdicts written by a read-only review of old tasks. Each element:

```json
{"key": "WG-312", "verdict": "resolved", "summary": "...", "evidence": ["..."],
 "rewrites": [{"title": "...", "description": "...", "files": ["..."], "acceptance_criteria": ["..."], "size": "one-file"}],
 "duplicate_of": [], "decision": null, "command": null}
```

| Verdict | Effect |
| --- | --- |
| `resolved`, `obsolete` | comment, tag `triage:<verdict>`, move to `cancelled` (reason `triage: <verdict>`) |
| `open` | one new task per rewrite (`follows_up` the original; priority, projects and repositories copied; tags = the original's minus `wg136` and `kind:*`, plus `triaged` and `scope:<size>` (what the change touches: one-file, one-crate, several-crates; legacy small/medium/large read as those)); each key in `duplicate_of` (open work an existing task already tracks) gets a `duplicates` relation from the original instead of a new task; then the original is commented, tagged `triage:superseded` and cancelled |
| `needs-decision` | comment with the decision, tags `needs-decision` and `triage:needs-decision`; status unchanged. `decision` is prose, or an object `{"question": "...", "options": [{"label": "...", "detail": "...", "recommended": true}]}` (an option may be a bare label string) that also opens a structured [decision](api.md#decisions) on the task |
| `needs-run` | comment with the `command` that confirms it, tag `triage:needs-run`; status unchanged; `command` is required |
| `unclear` | comment, tag `triage:unclear`; status unchanged |

- The whole file is validated before anything changes. An element with an unknown verdict or key, a missing key, an `open` with neither rewrites nor `duplicate_of`, a rewrite without title or description, a duplicate key, or a `cancelled` move the lifecycle refuses (for example an accepted task) is reported as an error and skipped; the other elements still apply.
- A task that already has a `triage:*` tag is reported as `skipped`, so re-running a file is safe.
- `--dry-run` prints the same table and errors as a real run and changes nothing.
- `--source` names who triaged (e.g. `triage batch WG-1215`) and appears in each comment as `Triage (<source>): ...`. Without it the comment reads `Triage: ...`.
- Output is a table (key, verdict, action, new keys or error). With `--json` the output is `{"applied": [...], "skipped": [...], "errors": [...]}`. The command exits 1 if any element errored.
- Each service call commits on its own, so an element that fails part way through keeps the steps before the failure; it is reported as an error.
- `TaskCreate` has no `category` field, so created tasks do not carry the original's category.

## Examples

```sh
uv run workgraph task create "Fix crash" --description @notes.md -c "no crash on expiry" -P high --ready -r api -t bug
uv run workgraph task list --status ready --priority high --repo api --search "token" --sort priority --asc
uv run workgraph --json task show WG-12
uv run workgraph task update WG-12 --status blocked --reason "waiting for infra" --expected-version 4
uv run workgraph task create "Add token expiry" --file src/auth/token.rs:refresh --file docs/auth.md
uv run workgraph task update WG-12 --add-file src/auth/session.rs --expected-version 5
uv run workgraph task list --file src/auth/token.rs
uv run workgraph task depend WG-13 WG-12
uv run workgraph task promote-ready --tag sprint-42 --dry-run
uv run workgraph task promote-ready --all --dry-run
uv run workgraph task accept WG-12 --manual --reason "reviewed" --promote-dependents
uv run workgraph task bulk-update WG-1 WG-2 WG-3 --add-tag sprint-42
uv run workgraph git sync --full
uv run workgraph git changes WG-12
uv run workgraph analytics query execution_duration --by provider --by model --since 2026-09-01
uv run workgraph analytics export facts.parquet --format parquet --dataset task_facts
uv run workgraph automation preview --rule "tag bugs" --task WG-12
uv run workgraph classify evaluate --provider ollama --model qwen2.5:7b-instruct
uv run workgraph verify run WG-12 --tier fast
```

All commands are single-line and work unchanged in PowerShell, bash, zsh and Nushell.

## UI screenshots

`workgraph ui screenshot PAGE -o FILE.png` loads a page of the web UI served by `workgraph serve` in a
headless browser and writes a PNG. `PAGE` is the UI path with its query string, so any filtered view
can be captured:

```sh
workgraph ui screenshot "graph?repo=rho&linked=1" -o rho-graph.png --width 2400 --height 1500 --scale 2
workgraph ui screenshot tasks/WG-12 -o wg-12.png
workgraph ui screenshot tasks -o tasks-phone.png --mobile --width 390 --height 844 --scale 3 --theme dark
```

`--mobile` emulates a touch phone (mobile viewport, touch input) and `--theme light|dark` emulates the
system colour scheme, so the phone layout and both themes can be checked without a device.

No browser is bundled: Chrome, Edge or Chromium is found on `PATH` or in its standard install location,
or named with `--browser` / `WORKGRAPH_BROWSER`. The capture waits until nothing on the page is marked
`aria-busy` and, for pages that load their own data, until that data has rendered (`--wait-for SELECTOR`
overrides this), then `--settle` seconds. The server address comes from the config unless `--url` is given.

The graph page reads its filters from the URL: `project`, `repo`, `open=1` (hide accepted and cancelled
tasks), `linked=1` (only tasks with a dependency, parent or relation edge) and `relations=0`. Tasks with no
dependency or parent edge are laid out in a grid below the dependency layers.
