# Rust rewrite: contract inventory

Companion to [rust-rewrite.md](rust-rewrite.md). This is the list of external contracts the Rust
binary must honour exactly. Section 13 was extracted from the running Python code at `5aecab8`
(the Typer command tree, the MCP server's `list_tools`/`list_resources`, the FastAPI OpenAPI
document, `sqlite_master` of a database migrated to head, and the configuration model's JSON
Schema). Sections 6 to 12 were collected from the source and its documentation by reading and
searching it.

Every row has an id (`C001`, `M01`, `H001`, `X1`, ...). Task acceptance criteria in
[rust-rewrite.md](rust-rewrite.md#task-breakdown) name the ids a task must satisfy, and the
conformance corpus tags each scenario step with the ids it exercises, so coverage of the inventory
can be counted.

The tables are a reading aid. The machine-checked form of each contract is the snapshot that task
CF-01 checks in under `contracts/` (`cli.json`, `mcp.json`, `openapi.json`, `schema-0001.sql` to
`schema-0010.sql`, `config-schema.json`), together with a test in the Python suite that fails when
the code and the snapshot disagree. Task CF-02 regenerates this file from those snapshots.

## Counts (measured at 5aecab8)

| Contract | Count |
| --- | --- |
| CLI leaf commands | 186, under 31 top-level names (`task` 24, `experiment` 16, `analytics` 13, `automation` 12, `git` 12, `claude` 11, `stack` 10, `attempt` 9, `classify` 9, `repo` 9, `run` 8, `job` 7, `project` 7, `workspace` 6, `config` 5, `verify` 5, `tracker` 3, `worker` 3, `agent` 2, `demo` 2, `events` 2, `export` 2, and `backup`, `doctor`, `import`, `init`, `mcp`, `restore`, `search`, `serve`, `ui` with one each) |
| CLI exit codes | 8 (0 to 7) plus 130 on interrupt and Click's 2 for usage errors |
| MCP tools | 66 |
| MCP resources | 2 static, 5 templates |
| HTTP operations in OpenAPI | 124 over 109 paths, 73 component schemas |
| HTTP routes outside OpenAPI | 7 (X1 to X7) |
| Database | 72 tables (65 ordinary, `alembic_version`, `task_fts` and its 5 FTS5 shadow tables), 207 indexes (104 named), 2 triggers, migration head `0010`, 10 revisions |
| Configuration keys | 38 leaf keys and 3 named-table families (`llm.providers.<name>`, `agents.<name>`, `trackers.<name>`) with 6 typed shapes; 41 rows in section 13.5 |
| Environment variables | 13 entries read (E01-E13, some naming a group of variables), 5 entries set for children (E20-E24, including the `WORKGRAPH_REPO_<NAME>_PATH` family) |
| Domain enums | 23 |
| Event-type and reason strings in the code | 111 |
| Job kinds | 17 |
| Permissions | 30 |
| Error codes | 12 |
| Python source | 45,173 lines in `src/workgraph` |
| Python tests | 666 test functions in 55 files (about 766 cases collected with parametrisation); 164 of the functions in 16 files drive an interface (CLI, MCP, HTTP, the hook subprocess), 502 in 39 files call services or pure functions in-process |


## 6. Environment variables

| # | Variable | Read by | Meaning |
| --- | --- | --- | --- |
| E01 | `WORKGRAPH_HOME` | `config/paths.py`, root `--home` | relocate all state under `<home>/config`, `<home>/data`, `<home>/logs` |
| E02 | `WORKGRAPH_WORKSPACE` | root `-w`, every `--workspace` option, hook | default workspace slug or id |
| E03 | `WORKGRAPH_ATTEMPT_ID` | `claude hook` | correlates a hook payload with the attempt that launched the session (`workgraph_launched`) |
| E04 | `WORKGRAPH_TASK_ID` | `claude hook` | correlates with a task only (`environment` binding) |
| E05 | `WORKGRAPH_BROWSER` | `ui screenshot` | browser executable for CDP screenshots |
| E06 | `WORKGRAPH_ALLOWED_HOSTS` | `api/security.py` | extra allowed `Host` values, comma separated |
| E07 | `WORKGRAPH_ALLOWED_ORIGINS` | `api/security.py` | extra allowed `Origin` values, comma separated |
| E08 | `CLAUDE_CONFIG_DIR` | `claude install/uninstall/status` | Claude Code configuration directory instead of `~/.claude` |
| E09 | names in `api_key_env`, `email_env`, `token_env` | LLM providers, Jira | credentials; the configuration names the variable, never the value |
| E10 | `PROGRAMFILES`, `PROGRAMFILES(X86)`, `LOCALAPPDATA` | `ui screenshot` | standard browser install locations on Windows |
| E11 | `LOCALAPPDATA`, `APPDATA`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `HOME` | `platformdirs` | default storage locations when `WORKGRAPH_HOME` is unset |
| E12 | `COLUMNS`, `LINES`, `NO_COLOR`, `FORCE_COLOR`, `TERM`, `TTY_COMPATIBLE`, `TTY_INTERACTIVE` | `rich` | width and colour of human output |
| E13 | `PATH`, `PATHEXT` | executable resolution | `git`, `claude`, agent executables, `workgraph` itself (`git install-hook`, `claude install`) |

Set for child processes:

| # | Variable | Set by | Value |
| --- | --- | --- | --- |
| E20 | `WORKGRAPH_ATTEMPT_ID`, `WORKGRAPH_TASK_ID`, `WORKGRAPH_TASK_KEY` | agent runs (`orchestration/runs.py`) | the attempt and task |
| E21 | `WORKGRAPH_MANIFEST` | agent runs, verification | path of `workgraph-manifest.json` |
| E22 | `WORKGRAPH_VERIFICATION_RUN_ID` | verification checks | the run id |
| E23 | `WORKGRAPH_REPO_<NAME>_PATH` | verification checks | checkout path per manifest repository (`<NAME>` upper-cased, non-alphanumerics replaced) |
| E24 | git: `GIT_TERMINAL_PROMPT=0`, `GIT_PAGER=cat`, `PAGER=cat`, `LC_ALL=C`, `LANG=C`, `GIT_OPTIONAL_LOCKS=0`, `GIT_DIR`/`GIT_WORK_TREE` removed; stack merges add author and committer `Workgraph Stack Integration <integration@workgraph.local>` | `integrations/git/cli.py`, `stack_integration.py` | inherited environment otherwise |

Child environments are minimal (`integrations/process.py` `_SAFE_ENV_KEYS`: `PATH`, `PATHEXT`,
`SYSTEMROOT`, `SYSTEMDRIVE`, `WINDIR`, `COMSPEC`, `TEMP`, `TMP`, `TMPDIR`, `HOME`, `USERPROFILE`,
`APPDATA`, `LOCALAPPDATA`, `PROGRAMDATA`, `PROGRAMFILES`, `PROGRAMFILES(X86)`, `LANG`, `LC_ALL`, `TERM`,
`USER` and the rest of that set) plus the configured `env`; the Claude Code adapter inherits the
whole environment.

## 7. Files and directories

`<config>`, `<data>` and `<logs>` are `platformdirs.user_config_dir("workgraph", appauthor=False)`,
`user_data_dir(...)` and `user_log_dir(...)` (on Windows `%LOCALAPPDATA%\workgraph`,
`%LOCALAPPDATA%\workgraph` and `%LOCALAPPDATA%\workgraph\Logs`), or `<home>/config`, `<home>/data`,
`<home>/logs` under `WORKGRAPH_HOME`.

| # | Path | Format |
| --- | --- | --- |
| F01 | `<config>/config.toml` | TOML, written as `tomli_w` writes it |
| F02 | `<config>/workspaces/<slug>.toml` | TOML |
| F03 | `<config>/api-token` | `secrets.token_urlsafe(32)` text, no newline, mode 0600 where supported |
| F04 | `<data>/workgraph.db` (+ `-wal`, `-shm`) | SQLite, section 4 |
| F05 | `<data>/analytics.duckdb` | analytics snapshot; internal, may be absent (the Rust version does not create it: design decision D12, divergence DV3) |
| F06 | `<data>/artifacts/...` | prompts, logs, reports; referenced from `artifacts` rows by relative path |
| F07 | `<data>/worktrees/<attempt>/<repo>`, `<data>/worktrees/stacks/<stack-id>/<repo>`, `.../bisect-<repo>` | managed git worktrees |
| F08 | `<data>/runs/...` | per-run request/manifest files (`workgraph-request.json`, `workgraph-manifest.json`) |
| F09 | `<data>/spool/claude-hooks/<received_at without : and ->-<12 hex>.json` | spooled hook records `{"received_at", "env": {attempt_id, task_id, workspace}, "payload", "spool_reason"?}`; `.failed` after five failed replays |
| F10 | `<data>/backups/*.zip`, `pre-restore-<timestamp>.db` | backup zip: `workgraph.db`, `config/config.toml`, `config/workspaces/*.toml`, optional `artifacts/`, `manifest.json` (schema revision, version) |
| F11 | `<logs>/...` | rotating JSON-lines log (`ts`, `level`, `logger`, `msg`, extras) |
| F12 | `~/.claude/settings.json` (or project/local scope) | hook handlers in exec form: `{"type": "command", "command": <workgraph exe>, "args": ["claude", "hook"], "timeout": N, "async": true}`, timestamped backup before each change |
| F13 | `<repo>/.git/hooks/post-commit` | optional hook from `git install-hook`, recognised by a marker so it is never overwritten |
| F14 | export `*.jsonl` | one record per line in foreign-key order (`export workspace`, `import`) |
| F15 | analytics export | CSV, JSON or Parquet (`analytics export --format`) |

## 8. Output formats

| # | Surface | Rule |
| --- | --- | --- |
| O01 | CLI `--json` success | `json.dumps(value, indent=2, default=str)` + `\n` on stdout: two-space indent, `", "`/`": "` separators, `ensure_ascii=True` (non-ASCII as `\uXXXX`), Python float repr, Pydantic `model_dump(mode="json")` for models (field declaration order; datetimes as `YYYY-MM-DDTHH:MM:SS[.ffffff]Z`), other datetimes `isoformat()` (`+00:00`), `Path` as `str` |
| O02 | CLI `--json` error | one line `{"error": {"code", "message", "details"}}` on stdout, compact separators, then the exit code (O05); `fail()` errors use code `cli_error` |
| O03 | CLI human output | `rich` tables (default box, bold header, `overflow="fold"`), `kv` blocks (`  key: value`, empty values skipped), `[dim]` "nothing to show" lines; no colour when stdout is not a terminal; `when()` dates as `%Y-%m-%d %H:%M` |
| O04 | CLI human error | stderr `error (<code>): <message>` and one `  key: value` line per detail except `errors` |
| O05 | Exit codes | 0 ok, 1 unexpected or `WorkgraphError` without mapping (incl. `git_error`), 2 `validation_failed`/`dependency_cycle` and Click usage errors, 3 `conflict`/`version_conflict`/`invalid_transition`/`resource_busy`, 4 `not_found`, 5 `permission_denied`, 6 `provider_unavailable`, 7 `config_error`, 130 interrupted; `task triage-apply` exits 1 when any element errored |
| O06 | HTTP JSON | Starlette `JSONResponse`: compact (`,`/`:`), `ensure_ascii=False`, `allow_nan=False`; errors `{"error": {code, message, details}}` with the status from the error class (404, 422, 409, 403, 503, 500, 400), request validation 422 `{"error": {"code": "validation_failed", "message": "invalid request", "details": {"errors": [<pydantic errors>]}}}` |
| O07 | HTTP headers | `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store` on API data; never any CORS header |
| O08 | SSE | `GET /api/v1/ws/{ws}/events/stream`: `event: domain` with the event JSON per new domain event, periodic `event: queue` job-count snapshots, ping comments every 15 s (sse-starlette framing) |
| O09 | MCP | tool results as structured content (the returned object) plus its JSON text; tool errors with `isError: true` and text `{"error": {...}}`; resources as `application/json` text |
| O10 | DB JSON columns | `json.dumps(value, sort_keys=True, separators=(",", ":"), default=str)` |
| O11 | DB timestamps | `YYYY-MM-DDTHH:MM:SS.ffffff+00:00`, always six fraction digits |
| O12 | Ids and keys | UUIDv4, lowercase, hyphenated; task keys `<PREFIX>-<n>` (tracker tasks keep the tracker key) |
| O13 | Event records (API/MCP/CLI) | `seq, id, workspace_id, event_type, entity_type, entity_id, actor, actor_kind, occurred_at, correlation_id, causation_id, attempt_id, schema_version, data`, with upcasting on read (`task.created` v1 to v2) |
| O14 | Logs | JSON lines on stderr and in `<logs>` (`ts`, `level`, `logger`, `msg`, extra fields) |
| O15 | `claude hook` | reads at most 8 MiB from stdin, prints nothing, always exits 0 |

## 9. Hook, agent and git protocols

| # | Protocol | Reference |
| --- | --- | --- |
| P01 | Claude Code hook ingestion: event names `SessionStart`, `SessionEnd`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `SubagentStart`, `SubagentStop`, `TaskCreated`, `TaskCompleted`, `Stop`; lenient payload validation, redaction, `dedupe_key` idempotency, spool on failure | [claude-code.md](claude-code.md) |
| P02 | Claude Code execution adapter: `claude -p --output-format stream-json --verbose --permission-mode <mode> --session-id <uuid> [--model] [--allowedTools] [--max-budget-usd] [--add-dir]`, prompt on stdin | [claude-code.md](claude-code.md) |
| P03 | Subagent transcript and `agent-<id>.meta.json` sidecar reading, usage dedupe per `message.id` | [claude-code.md](claude-code.md) |
| P04 | `workgraph.agent.v1` command agent protocol: request on stdin and in `workgraph-request.json`, manifest file, placeholders `{attempt_id}`, `{task_id}`, `{task_key}`, `{workdir}`, `{manifest}`, `{prompt_file}`, JSON-lines output `progress`/`log`/`usage`/`artifact`/`result` | [agent-protocol.md](agent-protocol.md) |
| P05 | Git conventions: attempt branches `wg/<task-key>/attempt-<n>-<id8>`, stack integration branches `wg/stack/<parent-key lowercased>`, `--no-ff --no-edit` merges as the integration identity, `update-ref` compare-and-swap landing, trailers `Task-ID:` and `Workgraph-Task:`, attribution confidences | [git.md](git.md) |
| P06 | Verification checks: argument vectors, scopes, tiers, baselines, parsers, `error_exit_codes`, check environment (E21 to E23) | [task-lifecycle.md](task-lifecycle.md) |
| P07 | Jira (read-only GET), Ollama (`/api/tags`, `/api/chat`, `/api/embed`), OpenAI-compatible HTTP | [design/tracker-sync.md](design/tracker-sync.md), [inference.md](inference.md) |
| P08 | `claude mcp-config` block `{"mcpServers": {"workgraph": {"type": "stdio", "command": <exe>, "args": ["mcp"]}}}` | [mcp.md](mcp.md) |

## 10. Domain vocabulary

Enums (`core/models/enums.py`), all serialised by value:

- `AccessMode`: `read_only`, `read_write`
- `ActorKind`: `human`, `agent`, `system`, `automation`
- `AttemptStatus`: `queued`, `preparing`, `running`, `verifying`, `succeeded`, `failed`, `cancelled`, `timed_out`, `abandoned`
- `AttributionSource`: `explicit`, `execution_context`, `worktree`, `trailer`, `message_inference`, `rewrite_reconciliation`
- `ChangeSetItemStatus`: `pending`, `committed`, `verified`, `failed`
- `ChangeSetStatus`: `open`, `partially_applied`, `applied`, `partially_verified`, `verified`, `failed`, `abandoned`
- `CheckKind`: `test`, `lint`, `typecheck`, `build`, `integration`, `custom`
- `ClassificationStatus`: `pending`, `succeeded`, `failed`
- `CommentKind`: `comment`, `progress`, `result`
- `ExperimentStatus`: `draft`, `running`, `closed`
- `FailureCause`: `too_big`, `missing_context`, `ambiguous_spec`, `coupling`, `gaming`, `gate_gap`, `harness`, `budget`, `wrong_model`, `other`
- `FailureCauseSource`: `human`, `agent`, `mechanical`
- `JobStatus`: `queued`, `running`, `succeeded`, `failed`, `dead`, `cancelled`
- `RelationType`: `relates_to`, `duplicates`, `follows_up`, `regression_of`, `similar_to`
- `StackItemState`: `pending`, `ready`, `merged`, `conflict`, `ejected`, `verified`
- `StackStatus`: `open`, `integrating`, `verifying`, `verified`, `landed`, `abandoned`
- `TaskPriority`: `critical`, `high`, `medium`, `low`
- `TaskStatus`: `backlog`, `ready`, `blocked`, `claimed`, `running`, `awaiting_verification`, `awaiting_review`, `accepted`, `failed`, `cancelled`
- `TrialOutcome`: `pass`, `fail`
- `TrialPhase`: `staircase`, `refine`, `control`, `repeat`, `live`
- `TrialStatus`: `planned`, `running`, `graded`, `invalid`
- `VerificationStatus`: `pending`, `running`, `passed`, `failed`, `error`, `skipped`
- `WorktreeStatus`: `preparing`, `active`, `released`, `removed`, `orphaned`

Task transitions (`core/policies/lifecycle.py`, `TASK_TRANSITIONS`): the table in the code is the
contract; entering `accepted` is reserved for acceptance, a tracker may close (`accepted`,
`cancelled`) from any status, and `ready`/`claimed`/`running` require satisfied dependencies.

Event-type and reason strings found in the code (111; task CF-06 confirms the set of event types
from the code and the corpus databases):

- task: `task.accepted` `task.archived` `task.became_ready` `task.change_surface_computed` `task.claimed` `task.comment_added` `task.commit_linked` `task.commit_unlinked` `task.completed` `task.created` `task.decomposed` `task.dependency_added` `task.dependency_removed` `task.escalated` `task.priority` `task.progress_reported` `task.rejected` `task.relation_added` `task.released` `task.reopened` `task.repository_linked` `task.repository_unlinked` `task.result_reported` `task.status_changed` `task.synced` `task.unarchived` `task.updated`
- attempt: `attempt.cancel_requested` `attempt.cancelled` `attempt.created` `attempt.failed` `attempt.outcome_captured` `attempt.progress` `attempt.status_changed` `attempt.subagent_bound` `attempt.succeeded` `attempt.workspace_prepared`
- stack: `stack.auto_accept` `stack.bisect_probe` `stack.created` `stack.integration_conflict` `stack.integration_merged` `stack.integration_removed` `stack.integration_skipped` `stack.item_added` `stack.item_ejected` `stack.item_released` `stack.item_removed` `stack.item_state_changed` `stack.land_requested` `stack.landed` `stack.landing_refused` `stack.parent_not_reviewable` `stack.parent_rejected` `stack.rebased` `stack.rebuilt` `stack.status_changed` `stack.verification_errored` `stack.verification_failed` `stack.verification_failed_on_base` `stack.verification_started` `stack.verified`
- repository: `repository.changes_detected` `repository.checkout_added` `repository.checkout_discovered` `repository.checkout_lost` `repository.checkout_removed` `repository.dependency_declared` `repository.health_changed` `repository.registered` `repository.removed` `repository.updated`
- verification: `verification.auto_accept` `verification.completed` `verification.config_created` `verification.config_disabled` `verification.config_enabled` `verification.config_updated` `verification.started`
- classification: `classification.completed` `classification.confirmed` `classification.disagreement` `classification.evaluated` `classification.failed` `classification.promoted` `classification.superseded`
- automation: `automation.max_depth` `automation.rule_created` `automation.rule_disabled` `automation.rule_enabled` `automation.rule_updated` `automation.run_completed`
- project: `project.created` `project.repository_added` `project.repository_removed` `project.updated`
- experiment: `experiment.closed` `experiment.created` `experiment.frozen` `experiment.protocol_updated`
- workspace: `workspace.archived` `workspace.created` `workspace.settings_changed`
- worktree: `worktree.created` `worktree.recovered` `worktree.removed`
- git: `git.commit_ingested` `git.commits_attributed` `git.sync_completed`
- other: `notification.created` `predictor.fitted`

Job kinds (`jobs/handlers.py` `HANDLERS`, 17): `git_sync`, `tracker_sync`, `analyze_commits`,
`compute_change_surface`, `classify_task`, `embed_task`, `evaluate_classifier`, `agent_run`, `verify`,
`stack_integrate`, `stack_verify`, `decompose_task`, `automation_action`, `generate_report`,
`failure_analysis`, `compact_telemetry`, `noop`. Resource keys: `gpu`, `agent:<name>`, `repo:<id>`,
`run`, `verify:<repository-id>`, `exclusive:stack:<stack-id>:<repository-id>`, `exclusive:*`.
Job payloads and idempotency keys are stored in the shared database, so they are a contract while
a Python worker and a Rust worker may both run (design section 7).

Permissions (`security/permissions.py`, 30): `workspace:read`, `workspace:admin`, `project:write`,
`repo:read`, `repo:register`, `repo:admin`, `task:read`, `task:write`, `task:claim`, `task:complete`,
`task:accept`, `task:admin`, `git:read`, `git:ingest`, `git:link`, `attempt:write`, `stack:write`,
`experiment:write`, `run:start`, `run:cancel`, `verify:run`, `automation:read`, `automation:trigger`,
`automation:manage`, `config:manage`, `analytics:read`, `classification:request`,
`classification:confirm`, `job:manage`, `telemetry:ingest`. Default agent set: the six read
permissions plus `task:write`, `task:claim`, `task:complete`, `attempt:write`, `git:link`,
`classification:request`, `automation:trigger`, `telemetry:ingest`. Humans and `system:` actors hold
all. The `permission_denied` message is `actor '<actor>' lacks permission(s): <sorted, comma separated>`.

Error codes (`core/errors.py`, 12): `workgraph_error` (400), `not_found` (404), `validation_failed`
(422), `conflict` (409), `version_conflict` (409), `invalid_transition` (409), `dependency_cycle`
(422), `permission_denied` (403), `provider_unavailable` (503), `git_error` (500), `config_error`
(400), `resource_busy` (409); plus `cli_error` from the CLI's `fail()`.

Actors: `human:<user>`, `agent:<name>`, `system:<component>` (`system:tracker-sync`,
`system:migration`, ...), `automation:<rule>`.

## 11. Migration chain

| Revision | Down | Change |
| --- | --- | --- |
| `0001` | - | initial schema (1,781 lines): every original table, FTS5 `task_fts` (`porter unicode61`), event immutability triggers |
| `0002` | `0001` | automation run inputs |
| `0003` | `0002` | stacks, stack repositories, stack items (partial unique index: one active stack per parent) |
| `0004` | `0003` | `stacks.land_request` |
| `0005` | `0004` | subagent telemetry columns |
| `0006` | `0005` | experiments and experiment trials |
| `0007` | `0006` | predictor fits |
| `0008` | `0007` | `external_items`, `task_links` |
| `0009` | `0008` | data migration: `tasks.external_refs` entries to `task_links` rows, then drop the column |
| `0010` | `0009` | `task_files` |

The version table is Alembic's `alembic_version(version_num)`. Every process start upgrades the
database to head (`Workgraph.__init__`), `init` creates it, `doctor` reports the revision, `restore`
migrates a restored database forward.

## 12. Behavioural contracts that are not visible in one call

| # | Behaviour | Where it is specified |
| --- | --- | --- |
| B01 | Service mutations write current state and append domain events in one transaction; external programs never run inside a write transaction | [architecture.md](architecture.md) |
| B02 | Optimistic concurrency with `expected_version` | [task-lifecycle.md](task-lifecycle.md) |
| B03 | Job queue: atomic claiming, leases and heartbeats, exponential backoff, dead letter, idempotency keys, resource-key limits, maintenance leader election by an expiring lease row, lost-worker detection after 2 minutes | [architecture.md](architecture.md), [backup.md](backup.md) |
| B04 | Stack integration, verification, bisection, ejection, landing and auto-accept | [git.md](git.md) |
| B05 | Verification acceptance and auto-accept rules; tracked-task ownership | [task-lifecycle.md](task-lifecycle.md) |
| B06 | Worktree recovery on worker start and `worker recover` | [backup.md](backup.md) |
| B07 | Retention compaction of telemetry payloads and run logs | [backup.md](backup.md) |
| B08 | Tracker sync scheduling and status mapping | [design/tracker-sync.md](design/tracker-sync.md) |
| B09 | Classification on create and on material description change (`llm.auto_classify`, `auto_embed`) | [inference.md](inference.md) |
| B10 | Security: loopback binding, LAN listener and pairing, bearer token, Host/Origin checks, no shell execution | [security.md](security.md) |
| B11 | Automation rules: triggers, depth limit, rate limits, manual triggers | [automation.md](automation.md) |
| B12 | Experiments: frozen protocols, trials, grading, predictor fits (seeded with Python's `random.Random`) | [data-model.md](data-model.md#experiments) |

## 13. Generated inventory

The sections below are generated from the snapshots.

### 13.1 CLI

Extracted from the Typer application (`workgraph.cli.main:app`): **186 leaf commands** under 31 top-level names. Notation: `ARG` required argument, `[ARG]` optional, `ARG...` variadic; option types are Click type names, `{a,b}` a choice, `*` repeatable, `=x` default, `env X` the environment variable it reads. Every command also accepts the global `--json` (removed from argv wherever it appears, before parsing) and `--help`.

Root options: `--home` path env WORKGRAPH_HOME; `--workspace/-w` text env WORKGRAPH_WORKSPACE; `--version`. `--version` prints `workgraph <version>` and exits 0. With no arguments the root help is printed (Typer `no_args_is_help`).

Fast path: `workgraph claude hook ...` is dispatched before the CLI parser is built (P01, O15).

| # | Command | Arguments and options | Summary |
| --- | --- | --- | --- |
| C001 | `workgraph agent add-command` | NAME; EXECUTABLE; `--arg` str *; `--timeout` float =1800; `--supports-usage/--no-supports-usage` | Register a generic command agent (workgraph.agent.v1 protocol). |
| C002 | `workgraph agent list` | - | Configured agent adapters, availability and capabilities. |
| C003 | `workgraph analytics bottlenecks` | `--since` datetime; `--until` datetime; `--top` int =15; `--workspace/-w` str env WORKGRAPH_WORKSPACE | What slows subagents down: time by class, the slowest calls with live-agent concurrency, and per-task wall time. |
| C004 | `workgraph analytics compare` | METRIC; A_SINCE; A_UNTIL; B_SINCE; B_UNTIL; `--by` str *; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Compare a metric between two periods. |
| C005 | `workgraph analytics coupling` | `--project` str; `--min-support` int =1; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Repository co-change matrix, declared vs observed relationships and coupling candidates. |
| C006 | `workgraph analytics estimate` | [TASK]; `--title` str; `--category` str; `--repo` str *; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Estimate from comparable historical tasks (or report insufficient data). |
| C007 | `workgraph analytics export` | TARGET; `--format` str =csv; `--dataset` str; `--metric` str; `--by` str *; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Export a fact table or metric result as CSV, JSON or Parquet. |
| C008 | `workgraph analytics failures` | `--persist/--no-persist`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Recurring failure patterns with linked evidence. |
| C009 | `workgraph analytics history` | `--category` str; `--repo` str; `--limit` int =30; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Historical accepted tasks with their measured characteristics. |
| C010 | `workgraph analytics metrics` | - | Catalog of metrics with their definitions. |
| C011 | `workgraph analytics overview` | `--period` str =week; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Dashboard bundle of the most common metrics. |
| C012 | `workgraph analytics query` | METRIC; `--by` str *; `--period` str =week; `--project/-p` str; `--repo/-r` str; `--category` str; `--provider` str; `--model` str; `--since` datetime; `--until` datetime; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Run a metric with grouping and filters. |
| C013 | `workgraph analytics retrospective` | `--days` int =14; `--project` str; `--repo` str; `--no-model/--no-no-model`; `--output/-o` path; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Generate a retrospective (deterministic statistics, optional model narrative). |
| C014 | `workgraph analytics subagents` | `--period` str =week; `--since` datetime; `--until` datetime; `--model` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Subagent runs by period and model, with linked task outcomes and task size bands. |
| C015 | `workgraph analytics time` | `--since` datetime; `--until` datetime; `--by` str =class; `--model` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Where subagent wall time went: share by tool class, or per model, task or agent description. |
| C016 | `workgraph attempt abandon` | ATTEMPT; `--reason/-m` str =agent stopped without submitting | Abandon an external attempt whose agent stopped without submitting: the task returns to ready. |
| C017 | `workgraph attempt compare` | FIRST; SECOND | Compare provenance of two attempts (config, inputs, manifests, outcomes). |
| C018 | `workgraph attempt create` | TASK; `--provider` str =external; `--model` str; `--session-id` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Record an attempt by an externally-run agent. |
| C019 | `workgraph attempt finish` | ATTEMPT_ID; STATUS; `--summary/-m` str =; `--error` str | Finish an externally-run attempt. |
| C020 | `workgraph attempt list` | `--task` str; `--status` str *; `--provider` str; `--limit` int =50; `--workspace/-w` str env WORKGRAPH_WORKSPACE | List attempts. |
| C021 | `workgraph attempt manifests` | ATTEMPT_ID | Exact revision manifests (baseline / outcome / verification) of an attempt. |
| C022 | `workgraph attempt show` | ATTEMPT_ID | Show an attempt with telemetry and provenance. |
| C023 | `workgraph attempt start` | TASK; `--agent-name` str; `--adapter` str =claude_code_subagent; `--model` str; `--isolation` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Claim a task, record an attempt and prepare its worktrees for an external agent. |
| C024 | `workgraph attempt submit` | ATTEMPT; `--failed`; `--summary/-m` str; `--error` str | Conclude an external managed attempt from the commits made in its worktrees. |
| C025 | `workgraph automation catalog` | - | Supported triggers, actions and condition operators. |
| C026 | `workgraph automation create` | SPEC; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Create a rule from a JSON specification. |
| C027 | `workgraph automation disable` | NAME; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Disable a rule. |
| C028 | `workgraph automation enable` | NAME; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Enable a rule. |
| C029 | `workgraph automation list` | `--workspace/-w` str env WORKGRAPH_WORKSPACE | List rules. |
| C030 | `workgraph automation notifications` | `--unread/--no-unread`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Local notifications emitted by rules. |
| C031 | `workgraph automation preview` | `--rule` str; `--spec` str; `--event` str; `--task` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Dry run: evaluate conditions and show planned actions without executing them. |
| C032 | `workgraph automation process` | - | Process pending events and due schedules now (normally done by the worker). |
| C033 | `workgraph automation runs` | `--rule` str; `--status` str; `--limit` int =50; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Trigger and execution history. |
| C034 | `workgraph automation show` | NAME; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show a rule definition. |
| C035 | `workgraph automation trigger` | NAME; `--task` str; `--input` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Manually trigger a rule. |
| C036 | `workgraph automation update` | NAME; SPEC; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Replace a rule's specification. |
| C037 | `workgraph backup` | `--output/-o` path; `--include-artifacts/--no-include-artifacts` | Create a consistent online backup (database + configuration). |
| C038 | `workgraph classify confirm` | TASK; CATEGORY; `--note` str =; `--complexity` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Record a human classification (authoritative; also added to the gold set). |
| C039 | `workgraph classify disagreements` | `--workspace/-w` str env WORKGRAPH_WORKSPACE | Tasks whose classifications disagree across models or with humans. |
| C040 | `workgraph classify evaluate` | `--provider` str; `--model` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Evaluate a provider/model against the gold set (accuracy, per-class metrics, confusion matrix). |
| C041 | `workgraph classify evaluations` | `--workspace/-w` str env WORKGRAPH_WORKSPACE | Past evaluations across models and prompt versions. |
| C042 | `workgraph classify gold` | `--category` str; `--task` str; `--title` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | List the gold-standard set, or add a label to it. |
| C043 | `workgraph classify promote` | CLASSIFICATION_ID; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Make an alternative model classification current. |
| C044 | `workgraph classify reclassify` | `--provider` str; `--model` str; `--promote/--no-promote`; `--category` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Bulk re-classification as a new batch (previous results are kept). |
| C045 | `workgraph classify request` | TASK; `--provider` str; `--model` str; `--promote/--no-promote`; `--now/--no-now`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Classify a task (queued as a background job unless --now). |
| C046 | `workgraph classify show` | TASK; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Current classification and its history. |
| C047 | `workgraph claude backfill-time` | `--limit` int | Store the wall-time breakdown from transcripts on subagent runs that lack it (idempotent). |
| C048 | `workgraph claude backfill-transcripts` | `--limit` int | Fill subagent model, usage and task links from transcripts (idempotent). |
| C049 | `workgraph claude bind` | TASK; `--session` str; `--cwd` path; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Bind externally-launched Claude Code sessions to a task. |
| C050 | `workgraph claude hook` | `--debug` | Ingest one Claude Code hook payload from stdin (used by Claude Code; always exits 0). |
| C051 | `workgraph claude install` | `--scope` str =user; `--project-dir` path; `--command` str; `--event` str *; `--mcp` | Merge Workgraph's passive hooks into Claude Code settings (other settings are preserved). |
| C052 | `workgraph claude mcp-config` | `--command` str | Print the MCP client configuration block for Workgraph (stdio). |
| C053 | `workgraph claude replay` | `--limit` int =1000 | Ingest spooled hook events that could not be stored immediately. |
| C054 | `workgraph claude sessions` | `--limit` int =30 | Recent Claude Code sessions and their Workgraph bindings. |
| C055 | `workgraph claude status` | `--scope` str =user; `--project-dir` path | Hook installation state, ingestion statistics, spool backlog and recent sessions. |
| C056 | `workgraph claude unbind` | `--binding` str; `--cwd` path | Deactivate session bindings. |
| C057 | `workgraph claude uninstall` | `--scope` str =user; `--project-dir` path | Remove only Workgraph-owned hook entries. |
| C058 | `workgraph config migrate` | - | Rewrite configuration files in the current schema version. |
| C059 | `workgraph config path` | - | Print configuration and data locations. |
| C060 | `workgraph config set` | KEY; VALUE; `--workspace/-w` str | Set a configuration value (validated before it is written). |
| C061 | `workgraph config show` | `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show the effective configuration (global merged with workspace overrides). |
| C062 | `workgraph config validate` | - | Validate all configuration files against their schema. |
| C063 | `workgraph demo e2e` | `--home` path; `--keep/--no-keep` | Run the 25-step cross-repository demonstration in an isolated home (offline). |
| C064 | `workgraph demo seed` | `--workspace` str =demo; `--force/--no-force` | Populate demo data (real repos, tasks, runs, analytics) for exploring the web UI. |
| C065 | `workgraph doctor` | `--verify-events/--no-verify-events` | Check system health: storage, schema, Git, providers, workers and integrations. |
| C066 | `workgraph events list` | `--entity` str; `--prefix` str; `--after` int; `--limit` int =50; `--workspace/-w` str env WORKGRAPH_WORKSPACE | List events (filter by entity, type prefix or sequence). |
| C067 | `workgraph events verify` | `--workspace/-w` str env WORKGRAPH_WORKSPACE | Replay task events and compare with current-state tables. |
| C068 | `workgraph experiment close` | EXPERIMENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Close a running experiment; its trials no longer change. |
| C069 | `workgraph experiment create` | SLUG; `--title` str required; `--question` str; `--protocol` str required; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Create a draft experiment. Its protocol can change until it is frozen. |
| C070 | `workgraph experiment fit` | `--model` str required; `--repo-penalty` float =1.0; `--feature-penalty` float =0.01; `--samples` int =200; `--seed` int =0; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Fit the success predictor for one model over all graded trials, and store the fit. |
| C071 | `workgraph experiment freeze` | EXPERIMENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Validate the protocol, freeze it and start the experiment. |
| C072 | `workgraph experiment list` | `--status` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | List experiments in the workspace. |
| C073 | `workgraph experiment next` | EXPERIMENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Plan the next trial: the pending trial, a newly created one, or why the experiment is done. |
| C074 | `workgraph experiment predict` | `--model` str required; `--task` str; `--repo` str; `--lines` float; `--files` float; `--packages` str; `--fit` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Predict the probability of success, with an interval, labelled as an inference. |
| C075 | `workgraph experiment replay create` | SHAS...; `--repo` str required; `--validate`; `--oracle-template` str; `--env` str *; `--timeout` float =1800.0; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Create one replay task per commit. Existing replay tasks for a commit are reported, not duplicated. |
| C076 | `workgraph experiment replay scan` | `--repo` str required; `--rev` str =HEAD; `--limit` int; `--min-lines` int =1; `--max-lines` int; `--include-excluded`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | List commits that could become replay tasks, with their size band and exclusion reasons. |
| C077 | `workgraph experiment report` | EXPERIMENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Summarise an experiment per arm: bands, size fit, usage, failure causes and invalid trials. |
| C078 | `workgraph experiment show` | EXPERIMENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show an experiment's protocol summary and its trials in order. |
| C079 | `workgraph experiment trial add` | EXPERIMENT; TASK; `--arm` str required; `--phase` str required; `--band` int; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Add a planned trial of a task to a running experiment. |
| C080 | `workgraph experiment trial classify` | TRIAL; `--cause` str required; `--source` str required; `--notes` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Record why a failed or invalid trial failed. |
| C081 | `workgraph experiment trial grade` | TRIAL; `--timeout` float =3600.0; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Run the oracle and gates in a started trial's clone, measure it, and record the outcome. |
| C082 | `workgraph experiment trial show` | TRIAL; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show one trial: its arm, models, status, outcome and failure cause. |
| C083 | `workgraph experiment trial start` | TRIAL; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Clone the replay base for a planned trial, run its setup, and print the brief to give the subagent. |
| C084 | `workgraph export analytics` | OUTPUT; `--dataset` str =task_facts; `--format` str =csv; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Export an analytics fact table (task_facts, attempt_facts, task_commits, task_files). |
| C085 | `workgraph export workspace` | OUTPUT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Export all records of a workspace as JSON Lines. |
| C086 | `workgraph git analyze` | REPOSITORY; `--limit` int =200; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Run structural (Tree-sitter) analysis on not-yet-analysed commits. |
| C087 | `workgraph git changes` | TASK; `--min-confidence` float =0.5; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Commits, cumulative churn and final change surface of a task. |
| C088 | `workgraph git cleanup-worktrees` | `--apply` | Remove released, clean Workgraph-managed worktrees (branches are kept). |
| C089 | `workgraph git diff` | TASK; `--repository` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Print the full diff for a task (read live from Git). |
| C090 | `workgraph git install-hook` | REPOSITORY; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Optional: add a post-commit hook that queues an incremental sync (never required). |
| C091 | `workgraph git link` | TASK; SHA; `--repository` str; `--note` str =; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Explicitly link a commit to a task. |
| C092 | `workgraph git log` | REPOSITORY; `--limit` int =30; `--linked`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Ingested commits with their task links. |
| C093 | `workgraph git show` | REPOSITORY; SHA; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Commit metadata, files, symbols, equivalences and task links. |
| C094 | `workgraph git surface` | TASK; `--workspace/-w` str env WORKGRAPH_WORKSPACE | (Re)compute the final change surface of an accepted task. |
| C095 | `workgraph git sync` | [REPOSITORY]; `--full`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Import new commits (incremental) or reconcile full history. |
| C096 | `workgraph git unlink` | TASK; SHA; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Remove an explicit link (inferred provenance remains as evidence). |
| C097 | `workgraph git worktrees` | `--status` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Managed worktrees and their lifecycle state. |
| C098 | `workgraph import` | SOURCE | Import a workspace export into this database (never overwrites an existing workspace). |
| C099 | `workgraph init` | `--workspace/-w` str =default; `--name` str; `--prefix` str =WG; `--no-workspace/--no-no-workspace` | Create local storage, configuration, the database schema and a first workspace. |
| C100 | `workgraph job cancel` | JOB_ID | Cancel a queued job, or request cancellation of a running one. |
| C101 | `workgraph job compact` | `--dry-run/--apply` =True | Apply telemetry retention now (hook payloads and run logs past the retention window). |
| C102 | `workgraph job enqueue` | KIND; `--payload` str ={}; `--key` str | Enqueue a job (e.g. `noop` to check that workers are alive, `git_sync`, `compact_telemetry`). |
| C103 | `workgraph job list` | `--status/-s` str *; `--kind` str; `--limit` int =50; `--offset` int =0 | List jobs (newest first). |
| C104 | `workgraph job retry` | JOB_ID | Re-queue a dead-lettered or cancelled job. |
| C105 | `workgraph job show` | JOB_ID | Show a job, its payload, result and run history. |
| C106 | `workgraph job stats` | - | Counts by status. |
| C107 | `workgraph mcp` | `--transport` str =stdio; `--agent-name` str =mcp-client; `--grant` str *; `--host` str =127.0.0.1; `--port` int =8766 | Run the MCP server (stdio for local clients such as Claude Code; http for Streamable HTTP). |
| C108 | `workgraph project add-repo` | REF; REPOSITORY; `--role` str =member; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Associate a repository with a project. |
| C109 | `workgraph project create` | SLUG; `--name` str; `--description` str =; `--repo/-r` str *; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Create a project, optionally spanning repositories. |
| C110 | `workgraph project list` | `--workspace/-w` str env WORKGRAPH_WORKSPACE; `--all` | List projects. |
| C111 | `workgraph project remove-repo` | REF; REPOSITORY; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Remove a repository from a project. |
| C112 | `workgraph project rename-term` | REF; `--from` str required; `--to` str required; `--keep` str *; `--dry-run`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Rename a word across a project: open tasks, every task's tags, the project, its repositories |
| C113 | `workgraph project show` | REF; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show a project summary. |
| C114 | `workgraph project update` | REF; `--name` str; `--description` str; `--status` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Update project fields. |
| C115 | `workgraph repo depend` | REF; DEPENDS_ON; `--description` str =; `--remove/--no-remove`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Declare that one repository depends on another. |
| C116 | `workgraph repo list` | `--workspace/-w` str env WORKGRAPH_WORKSPACE; `--all`; `--project/-p` str | List registered repositories. |
| C117 | `workgraph repo refresh` | REF; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Re-check checkouts, remotes and health. |
| C118 | `workgraph repo register` | PATH; `--name` str; `--description` str =; `--read-only`; `--branch` str *; `--project/-p` str *; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Register a local Git repository (or add another checkout of a known one). |
| C119 | `workgraph repo remove` | REF; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Remove a registration. The Git repository on disk is not touched. |
| C120 | `workgraph repo remove-checkout` | REF; PATH; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Forget one checkout path of a repository (files untouched). |
| C121 | `workgraph repo show` | REF; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Inspect a repository registration. |
| C122 | `workgraph repo status` | REF; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show working-tree state (branch, staged, unstaged, untracked) of the primary checkout. |
| C123 | `workgraph repo update` | REF; `--description` str; `--access` {read_only,read_write}; `--default-branch` str; `--track` str *; `--untrack` str *; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Update registration metadata. --track/--untrack edit the tracked branches git sync scans. |
| C124 | `workgraph restore` | ARCHIVE; `--with-config/--no-with-config`; `--force/--no-force` | Restore a backup (the current database is kept as a safety copy) and migrate it forward. |
| C125 | `workgraph run cancel` | ATTEMPT_ID; `--reason` str =cancelled by operator | Cancel a queued or running attempt. |
| C126 | `workgraph run decompose` | TASK; `--create/--no-create`; `--provider` str; `--model` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Propose (and optionally create) child tasks using the configured model. |
| C127 | `workgraph run get` | ATTEMPT_ID | Attempt state, job status and recent progress events. |
| C128 | `workgraph run rerun` | ATTEMPT_ID | Re-run with the recorded configuration and baseline revisions (outputs may differ). |
| C129 | `workgraph run retry` | ATTEMPT_ID; `--instructions` str | Start a new attempt after a failed one. |
| C130 | `workgraph run schedule` | `--dry-run/--no-dry-run`; `--explain` str | Run one scheduler pass (auto_execute tasks whose dependencies are accepted). |
| C131 | `workgraph run start` | TASK; `--agent` str; `--model` str; `--instructions` str; `--wait/--no-wait`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Queue an agent run for a task (requires execution to be enabled). |
| C132 | `workgraph run watch` | ATTEMPT_ID | Stream progress until the attempt finishes. |
| C133 | `workgraph search` | QUERY; `--mode` str =hybrid; `--project/-p` str; `--repo/-r` str; `--category` str; `--status/-s` str *; `--agent` str; `--model` str; `--path` str; `--limit` int =20; `--similar-to` str; `--reindex/--no-reindex`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Search tasks (full-text always; semantic when embeddings exist). |
| C134 | `workgraph serve` | `--host` str; `--port` int; `--worker/--no-worker` =True; `--mcp/--no-mcp` =True; `--allow-remote/--no-allow-remote`; `--lan/--local` | Run the local HTTP API, web UI and (optionally) a background worker. |
| C135 | `workgraph stack add` | PARENT; TASK; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Add a child task of the parent to its active stack. |
| C136 | `workgraph stack create` | PARENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Stack a parent task's children, branching from each repository's default branch now. |
| C137 | `workgraph stack eject` | PARENT; TASK; `--reason/-m` str required; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Eject a merged child known to cause the stack's failure, and rebuild without it (no bisection). |
| C138 | `workgraph stack history` | PARENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show the stack's events in order: merges, conflicts, verification, bisection, rebuilds and landing. |
| C139 | `workgraph stack integrate` | PARENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Queue merging the stack's ready children onto each repository's integration branch. |
| C140 | `workgraph stack list` | `--workspace/-w` str env WORKGRAPH_WORKSPACE; `--all` | List stacks in the workspace. |
| C141 | `workgraph stack readmit` | PARENT; TASK; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Queue an ejected or pending child for integration again, without re-running its fast gate. |
| C142 | `workgraph stack remove` | PARENT; TASK; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Remove a child task from the active stack (not once it is merged or verified). |
| C143 | `workgraph stack show` | PARENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show the most recent stack of a parent task. |
| C144 | `workgraph stack verify` | PARENT; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Queue full verification of the stack's integration tip (bisecting and ejecting on failure). |
| C145 | `workgraph task accept` | REF; `--manual`; `--reason/-m` str =; `--promote-dependents`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Accept completed work (requires passing verification, or --manual with a reason). |
| C146 | `workgraph task archive` | REF; `--undo`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Archive (or unarchive) a task. Records are never deleted. |
| C147 | `workgraph task bulk-update` | REFS...; `--status/-s` {backlog,ready,blocked,claimed,running,awaiting_verification,awaiting_review,accepted,failed,cancelled}; `--priority/-P` {critical,high,medium,low}; `--add-tag` str *; `--remove-tag` str *; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Apply one change to many tasks. |
| C148 | `workgraph task claim` | REF; `--agent` str; `--lease-minutes` int =120; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Claim a ready task. |
| C149 | `workgraph task comment` | REF; BODY; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Add a comment. |
| C150 | `workgraph task complete` | REF; `--summary/-m` str =; `--attempt` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Report completion (moves to awaiting verification/review, never directly to accepted). |
| C151 | `workgraph task contract-check` | REF; `--model` str required; `--target` float =0.8; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Triage a task: not_delegable, incomplete, split or leaf, with the predicted success as an inference. |
| C152 | `workgraph task create` | TITLE; `--description/-d` str =; `--criterion/-c` str *; `--priority/-P` {critical,high,medium,low}; `--ready`; `--tag/-t` str *; `--file` str *; `--project/-p` str *; `--repo/-r` str *; `--parent` str; `--depends-on` str *; `--assignee` str; `--agent` str; `--auto-execute/--no-auto-execute`; `--due` datetime; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Create a task. |
| C153 | `workgraph task depend` | REF; DEPENDS_ON; `--remove/--no-remove`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Add (or remove) a dependency: REF cannot proceed until DEPENDS_ON is accepted. |
| C154 | `workgraph task graph` | `--root` str; `--project/-p` str; `--repo/-r` str; `--open`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Print the dependency/hierarchy graph. |
| C155 | `workgraph task history` | REF; `--type` str *; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show the immutable event history of a task and its attempts. |
| C156 | `workgraph task link-repo` | REF; REPOSITORY; `--access` str; `--remove/--no-remove`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Associate a repository with a task. |
| C157 | `workgraph task list` | `--status/-s` {backlog,ready,blocked,claimed,running,awaiting_verification,awaiting_review,accepted,failed,cancelled} *; `--priority/-P` {critical,high,medium,low} *; `--project/-p` str; `--repo/-r` str; `--tag/-t` str *; `--file` str; `--category` str; `--assignee` str; `--parent` str; `--roots`; `--search/-q` str; `--archived`; `--sort` str =created_at; `--asc`; `--limit` int =50; `--offset` int =0; `--workspace/-w` str env WORKGRAPH_WORKSPACE | List tasks with filters, sorting and pagination. |
| C158 | `workgraph task progress` | REF; MESSAGE; `--percent` float; `--attempt` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Report progress on a task. |
| C159 | `workgraph task promote-ready` | `--tag/-t` str *; `--project/-p` str; `--dry-run`; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Move backlog tasks whose dependencies are all accepted to ready (skips parents and parked tasks). |
| C160 | `workgraph task ready` | `--project/-p` str; `--repo/-r` str; `--agent` str; `--limit` int =20; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Ready tasks whose dependencies are accepted, highest priority first. |
| C161 | `workgraph task reject` | REF; `--reason/-m` str required; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Reject completed work and send it back to ready. |
| C162 | `workgraph task relate` | REF; TARGET; `--type` {relates_to,duplicates,follows_up,regression_of,similar_to}; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Relate two tasks (relates_to, duplicates, follows_up, regression_of, similar_to). |
| C163 | `workgraph task release` | REF; `--reason` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Release a claim. |
| C164 | `workgraph task result` | REF; OUTCOME; `--summary/-m` str =; `--attempt` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Report an attempt result. |
| C165 | `workgraph task show` | REF; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Show task details, comments, rollup and blockers. |
| C166 | `workgraph task triage-apply` | PATH; `--dry-run`; `--source` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Apply a file of triage verdicts (resolved, obsolete, open, needs-decision, needs-run, unclear). |
| C167 | `workgraph task update` | REF; `--title` str; `--description/-d` str; `--status/-s` {backlog,ready,blocked,claimed,running,awaiting_verification,awaiting_review,accepted,failed,cancelled}; `--priority/-P` {critical,high,medium,low}; `--add-tag` str *; `--remove-tag` str *; `--file` str *; `--add-file` str *; `--clear-files`; `--parent` str; `--clear-parent/--no-clear-parent`; `--assignee` str; `--agent` str; `--auto-execute/--no-auto-execute`; `--metadata` str; `--expected-version` int; `--reason` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Update task fields and/or status (validated transitions). |
| C168 | `workgraph tracker list` | - | Configured tracker connections. |
| C169 | `workgraph tracker status` | [CONNECTION] | Cursor, last sync, tracked task counts and unresolved pull request repositories. |
| C170 | `workgraph tracker sync` | CONNECTION; `--full`; `--queue` | Mirror a connection's issues into tasks, now or as a queued job. |
| C171 | `workgraph ui screenshot` | [PAGE]; `--out/-o` path =WindowsPath('workgraph-screenshot.png'); `--width` int =1600; `--height` int =1000; `--scale` float =1.0; `--wait-for` str; `--settle` float =1.0; `--timeout` float =60.0; `--url` str; `--browser` str; `--theme` str; `--mobile` | Capture a page of the running web UI to a PNG with a headless browser. |
| C172 | `workgraph verify config` | NAME; `--scope-type` str =repository; `--scope` str; `--check` str * required; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Create or update an approved verification configuration (trusted commands only). |
| C173 | `workgraph verify configs` | `--workspace/-w` str env WORKGRAPH_WORKSPACE | List verification configurations. |
| C174 | `workgraph verify failures` | `--check` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Historical verification failures. |
| C175 | `workgraph verify run` | TASK; `--attempt` str; `--tier` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | Run the applicable checks now against the task's current revisions. |
| C176 | `workgraph verify runs` | `--task` str; `--workspace/-w` str env WORKGRAPH_WORKSPACE | List verification runs. |
| C177 | `workgraph worker list` | - | Show workers and their heartbeats. |
| C178 | `workgraph worker recover` | - | Recover expired job leases, lost workers and orphaned worktrees now. |
| C179 | `workgraph worker run` | `--concurrency` int; `--kind` str *; `--once/--no-once`; `--max-idle` float | Run a worker until interrupted (Ctrl+C stops gracefully). |
| C180 | `workgraph workspace archive` | REF | Archive a workspace (records are kept). |
| C181 | `workgraph workspace create` | SLUG; `--name` str; `--prefix` str =WG; `--description` str =; `--default` | Create a workspace. |
| C182 | `workgraph workspace list` | `--all` | List workspaces. |
| C183 | `workgraph workspace set` | REF; `--allowed-root` str * | Update workspace settings such as allowed repository roots. |
| C184 | `workgraph workspace show` | [REF] | Show a workspace summary. |
| C185 | `workgraph workspace use` | REF | Set the default workspace. |

### 13.2 MCP server

Server name `workgraph`, title `Workgraph`, version = package version, instructions text as in `src/workgraph/mcp/server.py` `INSTRUCTIONS`. **66 tools**, all registered with structured output (an object output schema), **2 static resources** and **5 resource templates**; no prompts. Parameters marked `!` are required. Errors are tool errors whose text is `{"error": {code, message, details}}`. The exact input and output JSON Schemas (descriptions, bounds such as `limit` 1..200, and titles such as `task_createArguments`) are part of the contract and are snapshotted by task CF-03. Transports: stdio (`workgraph mcp`), Streamable HTTP (`/mcp/` under `serve`, or `workgraph mcp --transport http`).

| # | Tool | Parameters | Description |
| --- | --- | --- | --- |
| M01 | `workspace_list` | - | List workspaces. |
| M02 | `workspace_get` | workspace: string\|null | Get a workspace with task counts, queue status and running attempts. |
| M03 | `project_list` | workspace: string\|null | List projects in a workspace. |
| M04 | `project_get` | !project: string, workspace: string\|null | Get a project summary: repositories, task counts and recent tasks. |
| M05 | `repository_list` | workspace: string\|null, project: string\|null | List registered repositories (optionally only those in a project). |
| M06 | `repository_get` | !repository: string, workspace: string\|null | Get a repository registration: checkouts, remotes, health, branches. |
| M07 | `repository_register` | !path: string, name: string\|null, read_only: boolean, workspace: string\|null | Register a local Git repository path (requires repo:register permission). |
| M08 | `repository_status` | !repository: string, workspace: string\|null | Working-tree status of a repository's primary checkout (branch, staged, unstaged). |
| M09 | `stack_get` | !parent: string, workspace: string\|null | Get a parent task's most recent stack: repositories with base SHAs and integration branches, and each child's stack state. |
| M10 | `stack_list` | workspace: string\|null, include_inactive: boolean | List stacks in a workspace (active stacks unless include_inactive is set). |
| M11 | `stack_history` | !parent: string, workspace: string\|null | A parent task's most recent stack in order: merges, conflicts, verification runs, bisect probes, ejections, rebuilds and landing. |
| M12 | `stack_create` | !parent: string, workspace: string\|null | Stack a parent task's children on a fresh integration branch cut from each repository's default branch (requires stack:write). |
| M13 | `stack_add_item` | !parent: string, !task: string, workspace: string\|null | Add a child task of the parent to its active stack (requires stack:write). |
| M14 | `stack_remove_item` | !parent: string, !task: string, workspace: string\|null | Remove a pending, ready, conflicting or ejected child from the active stack (requires stack:write). |
| M15 | `stack_integrate` | !parent: string, workspace: string\|null | Queue merging the stack's ready children onto each repository's integration branch (requires stack:write). |
| M16 | `stack_verify` | !parent: string, workspace: string\|null | Queue full verification of the stack's integration tip; a failure bisects and ejects the culprit (requires stack:write). |
| M17 | `experiment_list` | workspace: string\|null, status: string\|null | List experiments in a workspace, optionally only those in one status (draft, running, closed). |
| M18 | `experiment_get` | !experiment: string, workspace: string\|null | Get an experiment by slug or id: its protocol (arms with pinned models, bands, budget) and its trials in order, with models requested and actually run, outcomes and failure causes. |
| M19 | `experiment_next` | !experiment: string, workspace: string\|null | Plan the next trial of an experiment: returns the pending trial if one is planned or running, otherwise creates the next trial from the protocol, or reports done with the reason (requires experiment:write). |
| M20 | `experiment_report` | !experiment: string, workspace: string\|null | Summarise an experiment per arm: pass rates by band with Wilson intervals, the size fit with bootstrap intervals, token usage by outcome, failure causes and invalid trials. |
| M21 | `experiment_fit` | !model: string, repo_penalty: number, feature_penalty: number, samples: integer, seed: integer, workspace: string\|null | Fit the success predictor for one model over all graded trials of every experiment, bootstrap it, and store the fit with its data cutoff and trial ids (requires experiment:write). |
| M22 | `experiment_predict` | !model: string, task: string\|null, repository: string\|null, src_lines: number\|null, src_files: number\|null, packages: list[string]\|null, fit: string\|null, workspace: string\|null | Predict P(success) for a model with an interval, labelled as an inference and naming its fit. Give either task (reads metadata contract.predicted) or repository, src_lines, src_files and packages. The fit is the latest one for the model unless fit is given. |
| M23 | `trial_classify` | !trial: string, !cause: FailureCause, !source: FailureCauseSource, notes: string\|null, workspace: string\|null | Record the failure cause of a failed or invalid experiment trial and its source (requires experiment:write). |
| M24 | `trial_start` | !trial: string, workspace: string\|null | Start a planned experiment trial: clone the task's replay base into a fresh single-branch clone (no remote), run the protocol's setup, and return the worktree, branch, base sha and the brief to give the subagent (requires experiment:write). |
| M25 | `trial_grade` | !trial: string, timeout: number, workspace: string\|null | Grade a started experiment trial: measure its clone against the base, run the oracle and gates, and record the outcome. A failure with flags (tests modified, stubs) is classed as gaming (requires experiment:write). |
| M26 | `task_create` | !title: string, description: string, acceptance_criteria: list[string]\|null, priority: TaskPriority, ready: boolean, tags: list[string]\|null, files: list[TaskFileIn]\|null, projects: list[string]\|null, repositories: list[string]\|null, parent: string\|null, depends_on: list[string]\|null, workspace: string\|null | Create a task. Repositories/projects/dependencies may be given by name or key. |
| M27 | `task_get` | !task: string, workspace: string\|null | Get full task details: description, criteria, files, comments, dependencies, rollup. |
| M28 | `task_contract_check` | !task: string, !model: string, target: number, workspace: string\|null | Triage a task for delegation: verdict not_delegable (needs a decision, no checkable oracle, or a report or register item), incomplete (metadata contract missing fields), split (predicted success below target) or leaf. Reports the missing fields, the reasons and the predicted success, an inference from the latest fit for the model. |
| M29 | `task_search` | !query: string, mode: {hybrid,lexical,semantic}, limit: integer, workspace: string\|null | Search tasks by text (full-text, plus semantic when embeddings exist). |
| M30 | `task_list` | statuses: list[TaskStatus]\|null, priorities: list[TaskPriority]\|null, project: string\|null, repository: string\|null, tags: list[string]\|null, file: string\|null, category: string\|null, parent: string\|null, text: string\|null, sort: {created_at,updated_at,priority,key,title,status,due_at}, descending: boolean, limit: integer, offset: integer, workspace: string\|null | List tasks with filters, sorting and pagination. |
| M31 | `task_update` | !task: string, expected_version: integer\|null, title: string\|null, description: string\|null, status: {backlog,ready,blocked,cancelled}\|null, priority: TaskPriority\|null, add_tags: list[string]\|null, remove_tags: list[string]\|null, acceptance_criteria: list[string]\|null, files: list[TaskFileIn]\|null, add_files: list[TaskFileIn]\|null, reason: string\|null, workspace: string\|null | Update task fields or status. Pass expected_version for optimistic concurrency. |
| M32 | `task_claim` | !task: string, lease_minutes: integer, workspace: string\|null | Claim a ready task for this agent (exclusive, with a lease). |
| M33 | `task_release` | !task: string, reason: string\|null, workspace: string\|null | Release a claimed task back to ready. |
| M34 | `task_complete` | !task: string, summary: string, attempt_id: string\|null, workspace: string\|null | Report completion. The task moves to awaiting verification/review, never to accepted. |
| M35 | `task_get_ready` | project: string\|null, repository: string\|null, limit: integer, workspace: string\|null | Ready, unclaimed tasks whose dependencies are all accepted, highest priority first. |
| M36 | `task_promote_ready` | tags: list[string]\|null, project: string\|null, dry_run: boolean, workspace: string\|null | Move backlog tasks whose dependencies are all accepted to ready, in one step. Skips tasks with children and tasks tagged parked. Optional tags and project narrow the candidates; dry_run lists the keys that would move without changing anything. |
| M37 | `task_add_dependency` | !task: string, !depends_on: string, workspace: string\|null | Make TASK depend on DEPENDS_ON (cycles are rejected). |
| M38 | `task_remove_dependency` | !task: string, !depends_on: string, workspace: string\|null | Remove a dependency. |
| M39 | `task_add_relation` | !task: string, !target: string, relation_type: RelationType, workspace: string\|null | Relate two tasks (relates_to, duplicates, follows_up, regression_of, similar_to). |
| M40 | `task_add_comment` | !task: string, !body: string, workspace: string\|null | Add a comment to a task. |
| M41 | `task_get_history` | !task: string, limit: integer, workspace: string\|null | Immutable event history of a task and its attempts. |
| M42 | `task_get_attempts` | !task: string, limit: integer, offset: integer, workspace: string\|null | Attempts made on a task (including failed and nested attempts). |
| M43 | `task_link_repository` | !task: string, !repository: string, read_only: boolean, workspace: string\|null | Associate a repository with a task. |
| M44 | `task_link_commit` | !task: string, !sha: string, repository: string\|null, note: string, workspace: string\|null | Explicitly link a commit (SHA or ref) to a task with confidence 1.0. |
| M45 | `task_report_progress` | !task: string, !message: string, percent: number\|null, attempt_id: string\|null, workspace: string\|null | Report progress (message and optional percent) on a task or attempt. |
| M46 | `task_report_result` | !task: string, !outcome: {succeeded,failed}, summary: string, attempt_id: string\|null, input_tokens: integer\|null, output_tokens: integer\|null, workspace: string\|null | Report an attempt outcome. Include usage only if your runtime measured it. |
| M47 | `attempt_create` | !task: string, model: string\|null, session_id: string\|null, parent_attempt: string\|null, workspace: string\|null | Register an attempt for work you are doing on a claimed task. |
| M48 | `attempt_start` | !task: string, model: string\|null, adapter: string, isolation: string\|null, workspace: string\|null | Claim a ready task (or continue your claim), record an attempt and prepare its managed git worktrees (stack base included). Returns the checkouts to work in. Requires task:claim and attempt:write. |
| M49 | `attempt_submit` | !attempt: string, status: string, summary: string\|null, error: string\|null | Conclude an external managed attempt after committing your work in the checkouts from attempt_start: captures the commits, runs the fast checks or verification and moves the task to review. Requires task:claim, attempt:write and verify:run. |
| M50 | `attempt_get` | !attempt_id: string | Get an attempt with telemetry, manifests and provenance. |
| M51 | `attempt_list` | task: string\|null, statuses: list[string]\|null, provider: string\|null, limit: integer, offset: integer, workspace: string\|null | List attempts in a workspace (filter by task, status or provider). |
| M52 | `run_start` | !task: string, agent: string\|null, model: string\|null, workspace: string\|null | Start a Workgraph-managed agent run (requires run:start and execution enabled). |
| M53 | `run_get` | !attempt_id: string, events: integer | Get a run: attempt state, job status and recent progress events. |
| M54 | `run_cancel` | !attempt_id: string, reason: string | Cancel a queued or running agent run. |
| M55 | `git_get_changes` | !task: string, min_confidence: number, workspace: string\|null | Commits linked to a task with provenance, cumulative churn and final change surface. |
| M56 | `git_get_task_diff` | !task: string, repository: string\|null, max_bytes: integer, workspace: string\|null | Full diff for a task, read live from Git (truncated to max_bytes). |
| M57 | `classification_get` | !task: string, workspace: string\|null | Current classification and full history for a task (model results are inferences). |
| M58 | `classification_request` | !task: string, workspace: string\|null | Queue a background classification job for a task. |
| M59 | `analytics_query` | !metric: string, group_by: list[{period,project,repository,category,priority,status,provider,model,agent,kind,check_name,module,file_class,relation_type}]\|null, period: {day,week,month,quarter,year}, project: string\|null, repository: string\|null, category: string\|null, provider: string\|null, model: string\|null, workspace: string\|null | Run a predefined metric with structured grouping/filters (see analytics_report for the catalog). |
| M60 | `analytics_report` | !report: {catalog,overview,coupling,failures,estimate,history}, task: string\|null, workspace: string\|null | Named analytical reports: catalog, overview, coupling, failures, estimate or history. |
| M61 | `analytics_subagents` | period: {week,month}, since: string\|null, until: string\|null, model: string\|null, workspace: string\|null | Subagent runs over time by period and model, with the acceptance outcomes and size bands of their linked tasks. |
| M62 | `analytics_agent_time` | since: string\|null, until: string\|null, by: {class,model,task,description}, model: string\|null, workspace: string\|null | Where subagent wall time went: the measured share by tool class (or per model, task or agent description), with model time, waiting on background work and polling loops. |
| M63 | `analytics_bottlenecks` | since: string\|null, until: string\|null, top: integer, workspace: string\|null | What slows subagents down: time by class, the slowest calls with the number of subagent runs active at the time (machine saturation), and agent wall time per task. |
| M64 | `automation_list` | workspace: string\|null | List automation rules and their recent runs. |
| M65 | `automation_trigger` | !rule: string, task: string\|null, workspace: string\|null | Trigger a manual automation rule (rules are authored by humans). |

Resources:

| URI | Name | Description |
| --- | --- | --- |
| `workgraph://workspaces` | `res_workspaces` | All workspaces |
| `workgraph://analytics/catalog` | `res_catalog` | Definitions of every supported metric |
| `workgraph://workspaces/{workspace}/summary` | `res_workspace` | Workspace summary |
| `workgraph://workspaces/{workspace}/projects/{project}` | `res_project` | Project summary |
| `workgraph://workspaces/{workspace}/tasks/{task}` | `res_task` | Task details |
| `workgraph://workspaces/{workspace}/tasks/{task}/history` | `res_history` | Task execution history (events and attempts) |
| `workgraph://workspaces/{workspace}/analytics/{metric}` | `res_metric` | A supported analytics metric grouped by week |

### 13.3 HTTP API and web serving

From the FastAPI application (`create_app`); OpenAPI 3 document at `/api/openapi.json` (title `Workgraph`, version = package version): **109 paths, 124 operations**, 73 component schemas. The OpenAPI document is itself served, and is snapshotted as JSON by task CF-04. Body and response columns name the schema; `!` marks a required query parameter. Every operation can also answer 401, 403, 404, 409, 421, 422 or 503 with the error envelope.

| # | Method | Path | OK status | Query | Body | Response | Summary |
| --- | --- | --- | --- | --- | --- | --- | --- |
| H001 | GET | `/api/v1/health` | 200 | - | - | object | Health |
| H002 | GET | `/api/v1/meta` | 200 | - | - | object | Meta |
| H003 | GET | `/api/v1/workspaces` | 200 | - | - | list[WorkspaceOut] | List Workspaces |
| H004 | POST | `/api/v1/workspaces` | 201 | - | WorkspaceCreate | WorkspaceOut | Create Workspace |
| H005 | GET | `/api/v1/workspaces/{ws}` | 200 | - | - | WorkspaceSummary | Workspace Summary |
| H006 | GET | `/api/v1/ws/{ws}/projects` | 200 | include_archived | - | list[ProjectOut] | List Projects |
| H007 | POST | `/api/v1/ws/{ws}/projects` | 201 | - | ProjectCreate | ProjectOut | Create Project |
| H008 | GET | `/api/v1/ws/{ws}/projects/{ref}` | 200 | - | - | ProjectSummary | Project Summary |
| H009 | PATCH | `/api/v1/ws/{ws}/projects/{ref}` | 200 | - | ProjectUpdate | ProjectOut | Update Project |
| H010 | POST | `/api/v1/ws/{ws}/projects/{ref}/repositories` | 200 | - | RepoLink | ProjectOut | Project Add Repo |
| H011 | DELETE | `/api/v1/ws/{ws}/projects/{ref}/repositories/{repository}` | 200 | - | - | ProjectOut | Project Remove Repo |
| H012 | GET | `/api/v1/ws/{ws}/repositories` | 200 | include_archived,project | - | list[RepositoryOut] | List Repositories |
| H013 | POST | `/api/v1/ws/{ws}/repositories` | 201 | - | RepositoryRegister | RepositoryOut | Register Repository |
| H014 | GET | `/api/v1/ws/{ws}/repositories/{ref}` | 200 | - | - | RepositoryOut | Get Repository |
| H015 | PATCH | `/api/v1/ws/{ws}/repositories/{ref}` | 200 | - | RepositoryUpdate | RepositoryOut | Update Repository |
| H016 | DELETE | `/api/v1/ws/{ws}/repositories/{ref}` | 200 | - | - | RepositoryOut | Remove Repository |
| H017 | POST | `/api/v1/ws/{ws}/repositories/{ref}/refresh` | 200 | - | - | RepositoryOut | Refresh Repository |
| H018 | GET | `/api/v1/ws/{ws}/repositories/{ref}/status` | 200 | - | - | RepositoryStatus | Repository Status |
| H019 | POST | `/api/v1/ws/{ws}/repositories/{ref}/sync` | 200 | mode | - | any | Sync Repository |
| H020 | GET | `/api/v1/ws/{ws}/repositories/{ref}/commits` | 200 | limit,offset,linked_only,include_unreachable | - | any | Repository Commits |
| H021 | GET | `/api/v1/ws/{ws}/repositories/{ref}/commits/{sha}` | 200 | - | - | any | Commit Detail |
| H022 | GET | `/api/v1/ws/{ws}/worktrees` | 200 | status | - | any | List Worktrees |
| H023 | GET | `/api/v1/ws/{ws}/tasks` | 200 | status,priority,project,repository,tag,category,assignee,parent,roots,q,created_after,created_before,archived,sort,descending,limit,offset | - | Page_TaskOut_ | List Tasks |
| H024 | POST | `/api/v1/ws/{ws}/tasks` | 201 | - | TaskCreate | TaskOut | Create Task |
| H025 | POST | `/api/v1/ws/{ws}/tasks/bulk` | 200 | - | BulkUpdate | BulkResult | Bulk Update |
| H026 | GET | `/api/v1/ws/{ws}/ready` | 200 | project,repository,limit | - | list[TaskOut] | Ready |
| H027 | GET | `/api/v1/ws/{ws}/graph` | 200 | project,repository,root,include_closed | - | TaskGraph | Graph |
| H028 | GET | `/api/v1/ws/{ws}/tasks/{ref}` | 200 | - | - | TaskDetail | Get Task |
| H029 | PATCH | `/api/v1/ws/{ws}/tasks/{ref}` | 200 | - | TaskUpdate | TaskOut | Update Task |
| H030 | POST | `/api/v1/ws/{ws}/tasks/{ref}/claim` | 200 | - | ClaimRequest\|null | TaskOut | Claim |
| H031 | POST | `/api/v1/ws/{ws}/tasks/{ref}/release` | 200 | - | ReasonBody\|null | TaskOut | Release |
| H032 | POST | `/api/v1/ws/{ws}/tasks/{ref}/complete` | 200 | - | CompleteBody\|null | TaskOut | Complete |
| H033 | POST | `/api/v1/ws/{ws}/tasks/{ref}/accept` | 200 | - | AcceptBody | any | Accept |
| H034 | POST | `/api/v1/ws/{ws}/tasks/{ref}/reject` | 200 | - | RejectBody | any | Reject |
| H035 | GET | `/api/v1/ws/{ws}/tasks/{ref}/acceptances` | 200 | - | - | any | Acceptances |
| H036 | POST | `/api/v1/ws/{ws}/tasks/{ref}/comments` | 201 | - | CommentCreate | CommentOut | Comment |
| H037 | POST | `/api/v1/ws/{ws}/tasks/{ref}/progress` | 200 | - | ProgressReport | CommentOut | Progress |
| H038 | POST | `/api/v1/ws/{ws}/tasks/{ref}/result` | 200 | - | ResultReport | TaskOut | Result |
| H039 | POST | `/api/v1/ws/{ws}/tasks/{ref}/dependencies` | 200 | - | DependencyBody | TaskOut | Add Dependency |
| H040 | DELETE | `/api/v1/ws/{ws}/tasks/{ref}/dependencies/{depends_on}` | 200 | - | - | TaskOut | Remove Dependency |
| H041 | POST | `/api/v1/ws/{ws}/tasks/{ref}/relations` | 200 | - | RelationCreate | RelationOut | Add Relation |
| H042 | POST | `/api/v1/ws/{ws}/tasks/{ref}/repositories` | 200 | - | RepoBody | TaskOut | Link Repository |
| H043 | DELETE | `/api/v1/ws/{ws}/tasks/{ref}/repositories/{repository}` | 200 | - | - | TaskOut | Unlink Repository |
| H044 | POST | `/api/v1/ws/{ws}/tasks/{ref}/archive` | 200 | undo | - | TaskOut | Archive |
| H045 | GET | `/api/v1/ws/{ws}/tasks/{ref}/history` | 200 | limit | - | list[EventOut] | History |
| H046 | GET | `/api/v1/ws/{ws}/tasks/{ref}/attempts` | 200 | limit,offset | - | any | Task Attempts |
| H047 | GET | `/api/v1/ws/{ws}/tasks/{ref}/commits` | 200 | min_confidence | - | any | Task Commits |
| H048 | POST | `/api/v1/ws/{ws}/tasks/{ref}/commits` | 200 | - | LinkCommitBody | any | Link Commit |
| H049 | GET | `/api/v1/ws/{ws}/tasks/{ref}/changes` | 200 | min_confidence | - | any | Task Changes |
| H050 | GET | `/api/v1/ws/{ws}/tasks/{ref}/diff` | 200 | repository,max_bytes | - | any | Task Diff |
| H051 | GET | `/api/v1/ws/{ws}/tasks/{ref}/change-sets` | 200 | - | - | any | Change Sets |
| H052 | GET | `/api/v1/ws/{ws}/tasks/{ref}/classification` | 200 | - | - | any | Classification |
| H053 | POST | `/api/v1/ws/{ws}/tasks/{ref}/classification` | 200 | provider,model,promote | - | any | Request Classification |
| H054 | POST | `/api/v1/ws/{ws}/tasks/{ref}/classification/confirm` | 200 | - | ConfirmBody | any | Confirm Classification |
| H055 | GET | `/api/v1/ws/{ws}/tasks/{ref}/similar` | 200 | limit | - | any | Similar |
| H056 | GET | `/api/v1/ws/{ws}/tasks/{ref}/estimate` | 200 | - | - | any | Estimate |
| H057 | GET | `/api/v1/ws/{ws}/tasks/{ref}/verification` | 200 | - | - | any | Verification Runs |
| H058 | GET | `/api/v1/ws/{ws}/tasks/{ref}/checks` | 200 | - | - | any | Applicable Checks |
| H059 | POST | `/api/v1/ws/{ws}/tasks/{ref}/verify` | 200 | attempt_id,tier | - | any | Verify |
| H060 | POST | `/api/v1/ws/{ws}/tasks/{ref}/decompose` | 200 | create | - | any | Decompose |
| H061 | GET | `/api/v1/ws/{ws}/tasks/{ref}/schedulability` | 200 | - | - | any | Schedulability |
| H062 | GET | `/api/v1/ws/{ws}/stacks` | 200 | include_inactive | - | list[StackOut] | List Stacks |
| H063 | POST | `/api/v1/ws/{ws}/stacks` | 201 | - | StackCreate | StackOut | Create Stack |
| H064 | GET | `/api/v1/ws/{ws}/stacks/{ref}` | 200 | - | - | StackOut | Get Stack |
| H065 | POST | `/api/v1/ws/{ws}/stacks/{ref}/items` | 201 | - | StackItemAdd | StackOut | Add Stack Item |
| H066 | DELETE | `/api/v1/ws/{ws}/stacks/{ref}/items/{task}` | 200 | - | - | StackOut | Remove Stack Item |
| H067 | GET | `/api/v1/ws/{ws}/stacks/{ref}/history` | 200 | - | - | list[EventOut] | Stack History |
| H068 | GET | `/api/v1/ws/{ws}/tasks/{ref}/stacks` | 200 | - | - | list[StackOut] | Task Stacks |
| H069 | POST | `/api/v1/ws/{ws}/stacks/{ref}/integrate` | 202 | - | - | any | Integrate Stack |
| H070 | POST | `/api/v1/ws/{ws}/stacks/{ref}/verify` | 202 | - | - | any | Verify Stack |
| H071 | GET | `/api/v1/ws/{ws}/attempts` | 200 | task,status,provider,limit,offset | - | any | List Attempts |
| H072 | POST | `/api/v1/ws/{ws}/runs` | 201 | - | RunStart | any | Start Run |
| H073 | GET | `/api/v1/attempts/compare` | 200 | a!,b! | - | any | Compare Attempts |
| H074 | GET | `/api/v1/attempts/{attempt_id}` | 200 | events | - | any | Get Run |
| H075 | GET | `/api/v1/attempts/{attempt_id}/manifests` | 200 | - | - | any | Manifests |
| H076 | GET | `/api/v1/attempts/{attempt_id}/log` | 200 | tail | - | any | Attempt Log |
| H077 | POST | `/api/v1/attempts/{attempt_id}/cancel` | 200 | - | CancelBody\|null | any | Cancel Run |
| H078 | POST | `/api/v1/attempts/{attempt_id}/retry` | 200 | - | - | any | Retry Run |
| H079 | POST | `/api/v1/attempts/{attempt_id}/rerun` | 200 | - | - | any | Rerun |
| H080 | GET | `/api/v1/jobs` | 200 | workspace,status,kind,task,limit,offset | - | any | List Jobs |
| H081 | GET | `/api/v1/jobs/{job_id}` | 200 | - | - | any | Get Job |
| H082 | POST | `/api/v1/jobs/{job_id}/cancel` | 200 | - | - | any | Cancel Job |
| H083 | POST | `/api/v1/jobs/{job_id}/retry` | 200 | - | - | any | Retry Job |
| H084 | GET | `/api/v1/workers` | 200 | - | - | any | Workers |
| H085 | GET | `/api/v1/ws/{ws}/verification/configs` | 200 | - | - | any | Verification Configs |
| H086 | POST | `/api/v1/ws/{ws}/verification/configs` | 201 | - | VerificationConfigBody | any | Create Verification Config |
| H087 | POST | `/api/v1/verification/configs/{config_id}/{action}` | 200 | - | - | any | Toggle Verification Config |
| H088 | GET | `/api/v1/ws/{ws}/verification/runs` | 200 | status,limit | - | any | Verification Runs |
| H089 | GET | `/api/v1/verification/runs/{run_id}` | 200 | - | - | any | Verification Run |
| H090 | GET | `/api/v1/ws/{ws}/verification/failures` | 200 | check,repository,since | - | any | Verification Failures |
| H091 | GET | `/api/v1/automation/catalog` | 200 | - | - | any | Automation Catalog |
| H092 | GET | `/api/v1/ws/{ws}/automation/rules` | 200 | - | - | any | List Rules |
| H093 | POST | `/api/v1/ws/{ws}/automation/rules` | 201 | - | object | any | Create Rule |
| H094 | GET | `/api/v1/ws/{ws}/automation/rules/{ref}` | 200 | - | - | any | Get Rule |
| H095 | PUT | `/api/v1/ws/{ws}/automation/rules/{ref}` | 200 | - | object | any | Update Rule |
| H096 | POST | `/api/v1/ws/{ws}/automation/rules/{ref}/{action}` | 200 | - | object\|null | any | Rule Action |
| H097 | POST | `/api/v1/ws/{ws}/automation/preview` | 200 | - | PreviewBody | any | Preview Rule |
| H098 | GET | `/api/v1/ws/{ws}/automation/runs` | 200 | rule,status,limit | - | any | Automation Runs |
| H099 | GET | `/api/v1/ws/{ws}/notifications` | 200 | unread | - | any | Notifications |
| H100 | POST | `/api/v1/notifications/{notification_id}/read` | 200 | - | - | any | Read Notification |
| H101 | GET | `/api/v1/analytics/metrics` | 200 | - | - | any | Metric Catalog |
| H102 | POST | `/api/v1/ws/{ws}/analytics/query` | 200 | - | AnalyticsQuery | any | Analytics Query |
| H103 | GET | `/api/v1/ws/{ws}/analytics/overview` | 200 | period | - | any | Analytics Overview |
| H104 | POST | `/api/v1/ws/{ws}/analytics/compare` | 200 | - | CompareBody | any | Analytics Compare |
| H105 | GET | `/api/v1/ws/{ws}/analytics/coupling` | 200 | project,min_support | - | any | Analytics Coupling |
| H106 | GET | `/api/v1/ws/{ws}/analytics/failures` | 200 | - | - | any | Analytics Failures |
| H107 | GET | `/api/v1/ws/{ws}/analytics/agent-time` | 200 | since,until,by,model | - | any | Analytics Agent Time |
| H108 | GET | `/api/v1/ws/{ws}/analytics/bottlenecks` | 200 | since,until,top | - | any | Analytics Bottlenecks |
| H109 | GET | `/api/v1/ws/{ws}/analytics/history` | 200 | category,repository,limit | - | any | Analytics History |
| H110 | POST | `/api/v1/ws/{ws}/analytics/estimate` | 200 | - | EstimateBody | any | Analytics Estimate |
| H111 | POST | `/api/v1/ws/{ws}/analytics/export` | 200 | - | ExportBody | any | Analytics Export |
| H112 | GET | `/api/v1/ws/{ws}/reports` | 200 | - | - | any | Reports |
| H113 | POST | `/api/v1/ws/{ws}/reports/retrospective` | 201 | - | RetroBody | any | Retrospective |
| H114 | GET | `/api/v1/ws/{ws}/search` | 200 | q!,mode,project,repository,category,status,agent,model,path,created_after,created_before,limit | - | any | Search |
| H115 | GET | `/api/v1/ws/{ws}/classification/evaluations` | 200 | - | - | any | Evaluations |
| H116 | GET | `/api/v1/ws/{ws}/classification/disagreements` | 200 | - | - | any | Disagreements |
| H117 | GET | `/api/v1/settings` | 200 | workspace | - | any | Settings |
| H118 | PATCH | `/api/v1/settings` | 200 | - | SettingsPatch | any | Patch Settings |
| H119 | GET | `/api/v1/agents` | 200 | - | - | any | Agents |
| H120 | GET | `/api/v1/llm/health` | 200 | - | - | any | Llm Health |
| H121 | GET | `/api/v1/claude/status` | 200 | - | - | any | Claude Status |
| H122 | GET | `/api/v1/ws/{ws}/scheduler` | 200 | - | - | any | Scheduler |
| H123 | GET | `/api/v1/ws/{ws}/events` | 200 | after_seq,entity_type,entity_id,prefix,limit,descending | - | any | Events |
| H124 | GET | `/api/v1/ws/{ws}/events/stream` | 200 | after_seq,max_events | - | any | Event Stream |

Routes outside the OpenAPI document:

| # | Method | Path | Behaviour |
| --- | --- | --- | --- |
| X1 | GET | `/api/openapi.json` | the OpenAPI document (bearer token required, like every `/api` route) |
| X2 | GET | `/api/docs` | Swagger UI HTML (FastAPI's default page; it loads swagger-ui from the jsDelivr CDN) |
| X3 | GET | `/docs/oauth2-redirect` | Swagger UI OAuth2 redirect helper page |
| X4 | GET | `/pair` | pairing form for a LAN device; 303 to `/` for a trusted browser |
| X5 | POST | `/pair` | form field `code`; 303 and an `HttpOnly; SameSite=Strict` device cookie on success, 403 wrong code, 429 after 10 wrong codes |
| X6 | * | `/mcp/` | Streamable HTTP MCP transport (loopback clients only, bearer token, DNS-rebinding protection) |
| X7 | GET | `/{path}` | SPA: a static file of the built UI if one exists at that path, else `index.html` with `<meta name="workgraph-token" content=...>` inserted before the first `</head>` (or 303 to `/pair` for an unpaired LAN browser; a placeholder page when the UI is not built); `api/*` and `mcp*` paths give the JSON 404 envelope |

### 13.4 Database

SQLite file `<data>/workgraph.db`, WAL, `busy_timeout=15000`, `foreign_keys=ON`, `synchronous=NORMAL`, write transactions `BEGIN IMMEDIATE`. After migrating to head `0010`: **72 tables** (including `alembic_version` and the FTS5 table `task_fts` with its 5 shadow tables), **207 indexes** (103 of them automatic), **2 triggers**. The full `sqlite_master` text is the contract and is snapshotted by task CF-05; a database the Rust binary creates or migrates must give the same `sqlite_master` rows.

| Table | Columns | Named indexes |
| --- | --- | --- |
| `agent_attempts` | id, workspace_id, task_id, number, status, provider, adapter, agent_id, model, session_id, parent_attempt_id, retry_of_attempt_id, rerun_of_attempt_id, created_by, created_at, queued_at, started_at, finished_at, queue_wait_ms, duration_ms, exit_reason, exit_code, error, summary, input_task_version, input_task_snapshot, prompt_artifact_id, config_snapshot, environment, baseline_manifest_id, outcome_manifest_id, usage, usage_source, cost_usd, cost_source, telemetry, modified_repository_ids, retries, cancel_requested, updated_at | 3 |
| `agent_internal_tasks` | id, session_id, external_task_id, subject, description, status, task_id, external_agent_id, created_at, completed_at | 1 |
| `agent_profiles` | id, workspace_id, name, adapter, model, permissions, enabled, last_seen_at, created_at | 1 |
| `agent_sessions` | id, provider, external_session_id, workspace_id, attempt_id, task_id, binding_source, cwd, model, transcript_path, started_at, ended_at, end_reason, tool_calls, tool_failures, created_at, updated_at | 3 |
| `alembic_version` | version_num | 0 |
| `artifacts` | id, workspace_id, task_id, attempt_id, kind, name, media_type, path, size_bytes, sha256, retention, purged_at, metadata, created_at | 3 |
| `automation_rules` | id, workspace_id, name, description, enabled, trigger, conditions, actions, retry_max_attempts, max_depth, last_scheduled_at, next_run_at, created_by, version, created_at, updated_at | 1 |
| `automation_runs` | id, rule_id, workspace_id, trigger_type, trigger_key, trigger_event_id, depth, dry_run, status, matched, reason, condition_trace, action_results, error, attempts, correlation_id, created_at, finished_at | 2 |
| `change_set_items` | id, change_set_id, repository_id, base_sha, head_sha, branch, commit_shas, status, detail, updated_at | 1 |
| `change_sets` | id, workspace_id, task_id, attempt_id, title, status, created_at, updated_at | 2 |
| `classification_evaluations` | id, workspace_id, provider, model, prompt_version, taxonomy_version, sample_size, failures, accuracy, macro_f1, per_class, confusion, results, created_at | 1 |
| `classification_gold_labels` | id, workspace_id, task_id, title, description, category, labeled_by, created_at | 1 |
| `classifications` | id, task_id, source, status, provider, model, prompt_version, taxonomy_version, input_hash, category, subtype, domain, affected_systems, complexity, tags, confidence, rationale, raw_output, validation_status, error, duration_ms, is_current, confirmed_by, batch_id, created_by, created_at | 1 |
| `commit_analyses` | commit_id, analyzer_version, files_parsed, files_fallback, functions_affected, types_affected, detail, analyzed_at | 0 |
| `commit_equivalences` | id, repository_id, original_commit_id, equivalent_commit_id, kind, evidence, created_at | 1 |
| `commit_file_changes` | id, commit_id, path, old_path, change_type, similarity, insertions, deletions, is_binary, language, file_class, module | 2 |
| `commit_symbols` | id, file_change_id, symbol_kind, name, change_kind, start_line, end_line, lines_changed | 1 |
| `commit_task_links` | id, commit_id, task_id, source, confidence, evidence, attempt_id, created_by, created_at | 2 |
| `commits` | id, repository_id, sha, tree_sha, parents, author_name, author_email, authored_at, committer_name, committer_email, committed_at, subject, message, trailers, is_merge, patch_id, insertions, deletions, files_changed, reachable, unreachable_since, object_available, structure_analyzed, branches, ingested_at | 2 |
| `events` | seq, id, workspace_id, event_type, entity_type, entity_id, actor, actor_kind, occurred_at, correlation_id, causation_id, attempt_id, schema_version, data | 5 |
| `experiment_trials` | id, experiment_id, task_id, attempt_id, subagent_run_id, arm, requested_model, actual_model, phase, seq, band, features, base_ref, worktree_path, status, outcome, outcome_detail, failure_cause, failure_cause_source, notes, created_at, started_at, graded_at | 2 |
| `experiments` | id, workspace_id, slug, title, question, status, protocol, frozen_at, created_at, updated_at | 1 |
| `external_items` | id, workspace_id, task_id, connection, external_id, external_key, url, tracker_status, tracker_status_category, tracker_resolution, remote_updated_at, snapshot, snapshot_hash, synced_at, missing_since, created_at, updated_at | 2 |
| `findings` | id, workspace_id, kind, fingerprint, title, severity, metric_value, sample_size, evidence, first_detected_at, last_detected_at | 1 |
| `git_object_gaps` | id, repository_id, sha, context, detected_at | 0 |
| `hook_events` | id, dedupe_key, provider, hook_event_name, external_session_id, session_id, attempt_id, payload, payload_purged_at, received_at, source | 3 |
| `job_resource_claims` | job_id, resource_key | 1 |
| `job_runs` | id, job_id, attempt_number, worker_id, status, error, started_at, finished_at | 1 |
| `jobs` | id, workspace_id, kind, status, priority, payload, result, run_at, attempts, max_attempts, backoff_base_seconds, timeout_seconds, lease_owner, lease_expires_at, heartbeat_at, started_at, finished_at, cancel_requested, idempotency_key, resource_keys, last_error, error_kind, correlation_id, causation_id, task_id, attempt_id, created_by, created_at, updated_at | 5 |
| `managed_worktrees` | id, workspace_id, repository_id, source_checkout_id, attempt_id, path, branch, base_sha, status, last_error, created_at, updated_at, removed_at | 3 |
| `notifications` | id, workspace_id, level, title, body, source, data, read_at, created_at | 1 |
| `predictor_fits` | id, workspace_id, model, created_at, data_cutoff, trial_ids, feature_names, repositories, coef, penalty, n, converged, metrics, samples | 1 |
| `project_repositories` | project_id, repository_id, role, access_mode, created_at | 1 |
| `projects` | id, workspace_id, slug, name, description, status, settings, created_at, updated_at | 1 |
| `reports` | id, workspace_id, kind, title, scope, content_markdown, data, narrative_source, created_by, created_at | 1 |
| `repositories` | id, workspace_id, name, description, identity_fingerprint, default_branch, tracked_branches, access_mode, health_status, health_detail, health_checked_at, is_shallow, last_scanned_at, archived_at, metadata, created_at, updated_at | 2 |
| `repository_checkouts` | id, repository_id, path, kind, is_primary, access_mode, available, last_seen_at, created_at, updated_at | 2 |
| `repository_dependencies` | id, from_repository_id, to_repository_id, kind, description, created_at | 0 |
| `repository_refs` | id, repository_id, name, sha, updated_at | 1 |
| `repository_remotes` | id, repository_id, name, url, normalized_url, created_at, updated_at | 2 |
| `resource_leases` | resource_key, holder, purpose, acquired_at, expires_at | 0 |
| `revision_manifests` | id, workspace_id, attempt_id, kind, entries, created_at | 2 |
| `session_bindings` | id, workspace_id, task_id, provider, external_session_id, cwd_prefix, active, created_by, created_at | 2 |
| `stack_items` | id, stack_id, task_id, state, merge_order, detail, created_at, updated_at | 2 |
| `stack_repositories` | stack_id, repository_id, base_sha, integration_branch, created_at | 1 |
| `stacks` | id, workspace_id, parent_task_id, status, created_at, updated_at | 3 |
| `subagent_runs` | id, session_id, external_agent_id, agent_type, attempt_id, started_at, stopped_at, last_message_excerpt, transcript_path, requested_model, model, usage, telemetry, task_id | 2 |
| `system_cursors` | name, value, updated_at | 0 |
| `task_acceptances` | id, task_id, attempt_id, verification_run_id, decision, mode, reason, actor, manifest_id, created_at | 1 |
| `task_change_surfaces` | id, task_id, repository_id, attempt_id, base_sha, final_sha, files_changed, insertions, deletions, renames, binary_files, functions_affected, types_affected, detail, computed_at, updated_at | 1 |
| `task_comments` | id, task_id, attempt_id, author, kind, body, data, created_at | 2 |
| `task_dependencies` | id, task_id, depends_on_id, created_by, created_at | 2 |
| `task_embeddings` | id, task_id, provider, model, dimensions, vector, input_hash, created_at | 1 |
| `task_files` | task_id, position, path, anchor, note | 1 |
| `task_fts` | task_id UNINDEXED, workspace_id UNINDEXED, key, title, description, comments, tags, summary, tokenize='porter unicode61' | 0 |
| `task_fts_config` | k | 0 |
| `task_fts_content` | id | 0 |
| `task_fts_data` | id | 0 |
| `task_fts_docsize` | id | 0 |
| `task_fts_idx` | segid, | 0 |
| `task_links` | id, task_id, kind, provider, url, normalized_url, repo_full_name, number, repository_id, sources, title, created_by, created_at, updated_at | 3 |
| `task_projects` | task_id, project_id | 1 |
| `task_relations` | id, source_task_id, target_task_id, relation_type, created_by, created_at | 2 |
| `task_repositories` | task_id, repository_id, access, created_at | 1 |
| `task_tags` | task_id, tag | 1 |
| `tasks` | id, workspace_id, number, key, title, description, description_hash, acceptance_criteria, status, priority, category, parent_id, assignee, assigned_agent, claimed_by, claim_expires_at, auto_execute, estimated_effort, metadata, due_at, scheduled_at, created_by, version, created_at, updated_at, started_at, completed_at, accepted_at, archived_at, current_classification_id | 4 |
| `tool_events` | id, hook_event_id, session_id, attempt_id, external_agent_id, phase, tool_name, tool_use_id, duration_ms, error_excerpt, summary, occurred_at | 3 |
| `verification_check_results` | id, run_id, name, kind, required, repository_id, status, exit_code, duration_ms, timed_out, log_artifact_id, output_excerpt, failure_signatures, created_at | 1 |
| `verification_configs` | id, workspace_id, scope_type, scope_id, name, checks, enabled, approved_by, version, created_at, updated_at | 1 |
| `verification_runs` | id, workspace_id, task_id, attempt_id, manifest_id, config_snapshot, status, required_passed, triggered_by, started_at, finished_at, created_at | 3 |
| `workers` | id, hostname, pid, status, concurrency, kinds, current_jobs, jobs_completed, jobs_failed, started_at, heartbeat_at, stopped_at | 0 |
| `workspaces` | id, slug, name, description, key_prefix, next_task_number, settings, archived_at, created_at, updated_at | 0 |

Triggers: `events_no_delete` on `events`, `events_no_update` on `events` (reject UPDATE and DELETE on the append-only event log).

### 13.5 Configuration

TOML files: `<config>/config.toml` (global) and `<config>/workspaces/<slug>.toml` (per-workspace overrides, deep-merged over the global file). Unknown keys are rejected. `schema_version` 1 documents are upgraded in memory and rewritten by `config migrate`. `config set KEY VALUE` parses VALUE as JSON when it can and validates the whole document before an atomic write. Credentials are never in the files: only the names of environment variables (`api_key_env`, `email_env`, `token_env`).

| Key | Type | Default | Allowed |
| --- | --- | --- | --- |
| `schema_version` | integer | 2 |  |
| `default_workspace` | string\|null | null |  |
| `server.host` | string | "127.0.0.1" |  |
| `server.port` | integer | 8765 |  |
| `server.allow_non_loopback` | boolean | false |  |
| `server.lan` | boolean | true |  |
| `server.lan_address` | string\|null | null |  |
| `server.lan_networks` | array | null |  |
| `llm.providers.<name>` | table: OllamaProviderConfig or HeuristicProviderConfig or OpenAICompatibleProviderConfig |  |  |
| `llm.classification.provider` | string | null |  |
| `llm.classification.model` | string | null |  |
| `llm.embeddings.provider` | string | null |  |
| `llm.embeddings.model` | string | null |  |
| `llm.summarization.provider` | string | null |  |
| `llm.summarization.model` | string | null |  |
| `llm.auto_classify` | boolean | true |  |
| `llm.auto_embed` | boolean | true |  |
| `agents.<name>` | table: ClaudeCodeAgentConfig or CommandAgentConfig |  |  |
| `execution.enabled` | boolean | false |  |
| `execution.default_agent` | string | "claude_code" |  |
| `execution.max_concurrent_runs` | integer | 2 |  |
| `execution.max_attempts_per_task` | integer | 3 |  |
| `execution.auto_schedule` | boolean | false |  |
| `execution.isolation` | string | "worktree" | worktree,in_place |
| `execution.allow_push` | boolean | false |  |
| `execution.allow_merge` | boolean | false |  |
| `jobs.worker_concurrency` | integer | 4 |  |
| `jobs.gpu_concurrency` | integer | 1 |  |
| `jobs.per_agent_concurrency` | integer | 2 |  |
| `jobs.per_repository_concurrency` | integer | 1 |  |
| `jobs.verify_concurrency` | integer | 1 |  |
| `jobs.lease_seconds` | integer | 60 |  |
| `jobs.default_max_attempts` | integer | 5 |  |
| `stacks.auto_accept` | boolean | true |  |
| `verification.auto_accept` | boolean | true |  |
| `retention.telemetry_payload_days` | integer | 30 |  |
| `retention.run_log_days` | integer | 90 |  |
| `claude.capture_tool_payloads` | boolean | false |  |
| `claude.max_payload_chars` | integer | 4000 |  |
| `automation.max_depth` | integer | 3 |  |
| `trackers.<name>` | table: JiraTrackerConfig |  |  |

Typed sub-tables (selected by `type`); `!` marks a required key:

- `OllamaProviderConfig`: `type`="ollama", `base_url`="http://127.0.0.1:11434", `timeout_seconds`=120.0, `max_retries`=2, `backoff_seconds`=1.0, `api_key_env`=null
- `OpenAICompatibleProviderConfig`: `type`="openai_compatible", `base_url`!, `timeout_seconds`=120.0, `max_retries`=2, `backoff_seconds`=1.0, `api_key_env`=null
- `HeuristicProviderConfig`: `type`="heuristic"
- `ClaudeCodeAgentConfig`: `type`="claude_code", `executable`="claude", `model`=null, `permission_mode`="acceptEdits", `allowed_tools`!, `max_turns`=null, `max_budget_usd`=null, `timeout_seconds`=3600.0, `extra_args`!
- `CommandAgentConfig`: `type`="command", `executable`!, `args`!, `env`!, `timeout_seconds`=1800.0, `supports_progress`=true, `supports_usage`=false, `supports_resume`=false
- `JiraTrackerConfig`: `type`="jira", `flavour`="cloud", `base_url`!, `workspace`!, `projects`!, `email_env`=null, `token_env`!, `poll_interval_seconds`=300, `full_sync_interval_seconds`=86400, `overlap_seconds`=120, `missing_grace_seconds`=86400, `pr_fields`!, `dev_panel`=true, `cancelled_resolutions`!, `timeout_seconds`=30.0
