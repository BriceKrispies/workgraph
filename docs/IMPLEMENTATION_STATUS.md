# Implementation status

Status legend: **Done** = implemented *and* verified by the automated tests or executions named in
the evidence column. **Partial** = implemented with a stated limitation. Every row cites where it is
verified. Test paths are relative to `tests/`.

Environment used for verification: Windows 10, Python 3.12.9 (uv), Git 2.54, Node 24, pnpm 10.
Linux and macOS were not executed (the code uses `pathlib`, argument-vector subprocesses and
platform-specific process-tree termination for both families).

## 2-4. Stack, project setup, architecture

| Requirement | Status | Evidence |
| --- | --- | --- |
| uv project, `pyproject.toml`, `uv.lock`, console script `workgraph` | Done | `integration/test_cli.py::test_console_script_help`, `uv sync` |
| Ruff (lint + format), ty, pytest, pytest-asyncio | Done | `uv run tools/check.py` |
| Unified quality command | Done | `tools/check.py` |
| Pydantic v2, SQLAlchemy 2, Alembic, SQLite, DuckDB, FastAPI, Typer, MCP SDK, HTTPX, platformdirs, Git CLI, Tree-sitter | Done | used throughout; see module docs |
| React, TypeScript, Vite, pnpm, TanStack Query, React Flow, Recharts, Tailwind | Done | `web/` (`pnpm typecheck`, `pnpm test`, `pnpm build`, `pnpm test:e2e`) |
| Modular monolith, shared services across CLI/HTTP/MCP | Done | `docs/architecture.md`; same service calls in `cli/`, `api/routes/`, `mcp/server.py` |
| Structured logging, UTC timestamps, explicit exceptions, consistent result schemas | Done | `core/logging.py`, `persistence/types.py`, `core/errors.py`, `core/models/schemas.py` |
| Single-line, Nushell-compatible doc commands | Done | all docs |

## 5. Workspaces, projects, repositories

| Requirement | Status | Evidence |
| --- | --- | --- |
| Workspace/project/repository/checkout/task/attempt entities, many-to-many links | Done | `unit/test_tasks_core.py`, `integration/test_git.py::test_shared_repository_across_projects` |
| Identity independent of path/remote; multiple checkouts and remotes; relocation; no-remote repos | Done | `integration/test_git.py::test_register_identity_and_relocation`, `::test_repo_without_remote_and_removal_keeps_files`, `unit/test_foundations.py::test_normalize_remote_url` |
| Default/tracked branches, declared repository dependencies, read-only/read-write access, health checks | Done | `integration/test_git.py`, `integration/test_worktrees.py`, `integration/test_analytics.py` (declared+observed) |
| Register/list/inspect/refresh/remove without deleting repositories | Done | `integration/test_cli_groups.py`, `integration/test_git.py` |
| Workspace defaults and project overrides | Done | `unit/test_foundations.py::test_workspace_overrides` (workspace files override global), project `settings` JSON |
| UUID ids and readable keys (`WG-104`) | Done | `unit/test_tasks_core.py::test_create_and_get_task_with_key` |

## 6. Task management

| Requirement | Status | Evidence |
| --- | --- | --- |
| All task fields (description, criteria, status, priority, category, tags, projects, repos, parent/children, dependencies, relations, assignee/agent, timestamps, due/scheduled, effort, metrics, artifacts, comments, external refs, metadata) | Done | `core/models/schemas.py`, `unit/test_tasks_core.py` |
| Explicit lifecycle states, separate attempt states, validated transitions | Done | `core/policies/lifecycle.py`, `unit/test_tasks_core.py::test_status_transitions_are_validated` |
| Agent completion never implies acceptance | Done | `unit/test_tasks_core.py::test_agent_cannot_accept`, `integration/test_mcp.py` |
| Optimistic concurrency | Done | `unit/test_tasks_core.py::test_optimistic_concurrency`, `integration/test_api.py` (409) |
| Pagination, filtering, sorting, bulk operations, full-text search, archived retrieval | Done | `unit/test_tasks_core.py::test_filters_pagination_search_and_bulk` |
| Cross-project/cross-repository dependencies, cycle detection, workspace-global graph | Done | `unit/test_tasks_core.py::test_cycle_detection`, `::test_dependencies_block_and_unblock`, `::test_graph` |
| Parent-child rollups | Done | `unit/test_tasks_core.py::test_parent_child_rollup_and_hierarchy_cycle` |

## 7. Immutable history

| Requirement | Status | Evidence |
| --- | --- | --- |
| Append-only event log written atomically with state | Done | `unit/test_tasks_core.py::test_events_written_atomically_and_immutable`, `::test_failed_mutation_writes_no_event` |
| Event fields (id, type, entity, actor, timestamp, correlation, causation, attempt, schema version, data) | Done | `core/events/log.py` |
| History inspection and filtering | Done | `workgraph task history`, `workgraph events list`, `GET /ws/{ws}/events` |
| Schema versioning with upcasters; deterministic projection rebuild tests | Done | `core/events/log.py` (`task.created` v1->v2 upcaster), `unit/test_tasks_core.py::test_projection_rebuild_matches_tables`, `workgraph events verify` |
| Events cannot be modified by ordinary services | Done | SQLite triggers in migration `0001`; test above |

## 8-9. Attempts, telemetry, agent framework

| Requirement | Status | Evidence |
| --- | --- | --- |
| Attempt fields, multiple/failed/abandoned attempts, multi-repo attempts, nested/subagent attempts | Done | `integration/test_runs.py`, `integration/test_claude_hooks.py::test_session_correlation_tools_subagents_and_internal_tasks` |
| Measured vs reported values; no invented tokens/costs/durations | Done | `integration/test_runs.py::test_successful_cross_repo_run_with_verification` (`cost_usd is None`), `integration/test_analytics.py` (cost sample size 0) |
| Provider interface (registration, capabilities, config, execution, progress, cancellation, timeout, completion, session, usage) | Done | `integrations/agents/base.py`, `integration/test_runs.py` |
| Claude Code adapter (programmatic `stream-json`, sessions, cancellation, timeouts) | Partial | `integration/test_providers_and_services.py::test_claude_code_adapter_stream_json` uses a fake `claude` emitting documented messages; not run against the real CLI with credentials (would spend the user's quota) |
| Generic command adapter with validated templates and JSON I/O, no shell | Done | `integration/test_runs.py`, `integrations/agents/command.py` |
| Unavailable provider produces actionable error without state corruption | Done | `integration/test_runs.py::test_unavailable_provider_is_actionable_and_harmless`, `::test_claude_adapter_unavailable` |

## 10. MCP

| Requirement | Status | Evidence |
| --- | --- | --- |
| Official SDK, stdio and Streamable HTTP, structured I/O, resources, errors, discovery, pagination | Done | `integration/test_mcp.py::test_stdio_server_discovery_invocation_and_persistence`, `::test_streamable_http_transport` |
| All 41 required tools | Done | `integration/test_mcp.py::REQUIRED_TOOLS` |
| Tools use services; no raw SQL or shell | Done | `mcp/server.py` |
| Client integration test starting the real server and verifying persistence | Done | as above; also e2e demo steps 7-8 |
| Registration docs (Claude Code + another client) | Done | `docs/mcp.md` |

## 11. Claude Code lifecycle

| Requirement | Status | Evidence |
| --- | --- | --- |
| SessionStart/End, Pre/PostToolUse(+Failure), SubagentStart/Stop, TaskCreated/Completed, Stop | Done | `integration/test_claude_hooks.py` |
| ID mapping without authority; launch correlation; explicit binding | Done | `::test_session_correlation_tools_subagents_and_internal_tasks`, `::test_unbound_then_explicit_binding_by_cwd`, `integration/test_providers_and_services.py` |
| Hook CLI on stdin; validation; schema-version-aware parsing; idempotency; fast/nonblocking; spooling and replay; redaction | Done | `::test_idempotent_ingestion`, `::test_spooling_and_replay`, `::test_hook_command_end_to_end_subprocess`, `unit/test_foundations.py::test_redaction` |
| install/uninstall/status merging settings safely; passive hooks | Done | `::test_installer_merges_and_uninstalls_only_its_entries`, `::test_installer_refuses_invalid_json` |

## 12-14. Git, worktrees, structural analysis

| Requirement | Status | Evidence |
| --- | --- | --- |
| Scanning, incremental and full sync, metadata, trailers, parents, file stats, renames, binaries, merges | Done | `integration/test_git.py::test_commit_ingestion_stats_renames_and_incremental`, `::test_merge_commits`, `unit/test_foundations.py::test_parse_diff_tree_and_meta` |
| Rebased/amended/cherry-picked commits; rewritten history reconciliation with provenance | Done | `integration/test_git.py::test_rebase_reconciliation_and_cherry_pick` |
| Shallow detection; working-tree state (staged/unstaged/untracked) | Done | `integrations/git/cli.py` (`is_shallow`, `status_porcelain`), `integration/test_cli_groups.py` (`repo status`) |
| Attribution (explicit, execution context, worktree, trailers, labelled inference) with confidence; many-to-many; no double counting | Done | `integration/test_git.py::test_attribution_trailer_inference_and_ambiguity`, `::test_shared_commit_and_churn_vs_final_surface`, `integration/test_analytics.py` (shared commit counted once) |
| Diffs on demand, missing objects recorded, no stored payloads | Done | `GitService.task_diff`, `git_object_gaps` |
| Optional Git hook | Done | `workgraph git install-hook` |
| Execution manifests, managed worktrees, ownership, crash recovery, conflict prevention, parallel attempts | Done | `integration/test_worktrees.py` (all four tests) |
| Cross-repository change sets with partial states; exact revision manifests | Done | `integration/test_worktrees.py::test_prepare_worktrees_manifest_and_change_set`, `integration/test_runs.py` (verified) |
| Cumulative churn vs final change surface | Done | `integration/test_git.py::test_shared_commit_and_churn_vs_final_surface`, e2e step 19 |
| Tree-sitter for Python, C#, TypeScript, JavaScript, Go, Rust with fallback; functions/types, test vs production vs config/docs, repeated edits, module frequency, provenance | Done | `unit/test_foundations.py::test_symbol_extraction_all_languages`, `::test_analyze_file_changes_and_fallback`, `integration/test_git.py::test_structure_analysis_of_commits` |

## 15-17. Inference, classification, search

| Requirement | Status | Evidence |
| --- | --- | --- |
| Provider abstraction; Ollama over HTTP; model checks; timeouts; retries; JSON-schema output; validation; alternative providers | Done | `integration/test_llm_classification_search.py::test_ollama_structured_output_retries_and_errors` (real HTTP test server), `integration/test_providers_and_services.py::test_openai_compatible_provider` |
| Works without Ollama; model work in jobs; clear failure states | Done | `::test_provider_unreachable`, `::test_classification_job_dead_letters_when_model_unavailable` |
| Ollama classification against a real Ollama install | Partial | verified against an Ollama-API test server only; no Ollama was running in the verification environment |
| Provenance (provider, model, prompt/taxonomy version, input hash, output, confidence, duration, validation, errors); no silent replacement by another model | Done | `::test_classification_job_versioning_and_model_policy`, `::test_invalid_output_and_unavailable_model` |
| Auto-classification on create/material change; taxonomy; extra dimensions; manual confirmation | Done | same tests |
| Gold set, evaluation, bulk reclassification, disagreements, confusion matrices | Done | `::test_heuristic_provider_reclassify_and_evaluation`, `integration/test_cli_groups.py` |
| FTS5 search with filters; semantic search; model versioning; rebuild; hybrid retrieval; similar tasks; FTS without embeddings | Done | `::test_search_lexical_semantic_hybrid_and_similar` |

## 18-21. Jobs, automation, scheduler, verification

| Requirement | Status | Evidence |
| --- | --- | --- |
| Durable queue: scheduling, atomic claims, leases, heartbeats, retries/backoff, dead letter, cancellation, idempotency, concurrency/resource limits, crash recovery, history, worker shutdown | Done | `unit/test_jobs.py` (9 tests incl. concurrent claims) |
| Per-agent, per-repository and GPU limits | Done | `jobs/limits.py`, `unit/test_jobs.py::test_resource_limits` |
| Queue inspection via CLI and UI | Done | `workgraph job/worker`, web Automation/Execution pages |
| Automation: event/schedule/manual triggers, conditions, actions, retries, history, enable/disable, dry run, recursion prevention, provenance | Done | `integration/test_automation.py` (9 tests) |
| Scheduler: eligibility (deps, status, repos, resources, permissions, leases), priorities and fairness, concurrency, retries, escalation | Done | `integration/test_runs.py::test_scheduler_runs_independent_tasks_and_respects_dependencies`, `::test_escalation_after_attempt_limit` |
| LLM decomposition with validation and cycle rejection | Done | `integration/test_providers_and_services.py::test_decomposition_validation_and_creation` |
| Verification: configs by scope, tests/lint/types/build/integration, structured results, logs, timeouts, exit codes, required vs optional, approved commands only, revision manifests, failure history, acceptance conditions, manual acceptance, no merges/pushes | Done | `integration/test_runs.py::test_failed_verification_then_retry`, `::test_successful_cross_repo_run_with_verification`, `unit/test_foundations.py::test_check_spec_rejects_shell_unless_opted_in`, e2e steps 15-18 |
| Timeouts, cancellation (queued and running), crashed execution, lease recovery | Done | `integration/test_runs.py::test_timeout`, `::test_cancel_running_attempt`, `::test_cancel_queued_attempt`, `::test_agent_failure_crash_and_read_only_violation`, `unit/test_jobs.py::test_lease_expiry_recovery` |

## 22-27. Analytics, coupling, estimation, failures, retrospectives, provenance

| Requirement | Status | Evidence |
| --- | --- | --- |
| DuckDB analytics over read-only extraction, no downloads | Done | `analytics/warehouse.py`, `integration/test_analytics.py` |
| Throughput, execution, code, agent, quality, architecture metrics; filtering, grouping, period comparison, distributions (median/percentiles/n), definitions | Done | `integration/test_analytics.py::test_throughput_execution_and_agent_metrics` (percentiles checked against manual computation), `docs/analytics.md` |
| CSV/JSON/Parquet exports; structured queries only | Done | `integration/test_analytics.py::test_exports`, `::test_invalid_grouping_rejected` |
| Coupling: co-change matrix, declared vs observed, evidence per edge, candidates | Done | `integration/test_analytics.py::test_code_metrics_shared_commits_coupling_and_estimation` |
| Estimation with explanation, uncertainty, insufficient-data result, agent recommendations with bias caveat | Done | same test |
| Failure-pattern detection with linked evidence | Done | `integration/test_analytics.py::test_failures_and_retrospective_fallback` |
| Retrospectives with model narrative and deterministic fallback, citations | Done | `::test_failures_and_retrospective_fallback`, `::test_retrospective_with_model_narrative` |
| Provenance (task version, prompt artifact, config, environment, baselines, outcomes, verification config, tool summaries, artifacts, decisions); attempt comparison; rerun reporting missing objects | Done | `integration/test_runs.py::test_rerun_reproduces_inputs`, `orchestration/runs.py` |

## 28-30. Web UI, CLI, configuration and storage

| Requirement | Status | Evidence |
| --- | --- | --- |
| Overview, tasks, task detail, graph, repositories, execution, analytics, automation, settings; SSE updates; real data only | Done | `web/` Playwright suite (overview counts checked against the API; tasks, detail, graph, analytics, automation, execution, repositories, settings) |
| Demo-data command | Done | `workgraph demo seed`, `e2e/test_demo.py::test_demo_seed_populates_real_data` |
| CLI groups and subcommands, JSON output, help, exit codes | Done | `integration/test_cli.py`, `integration/test_cli_groups.py` |
| platformdirs storage, multiple workspaces, validated config files, env-var secrets, DB outside repos | Done | `config/paths.py`, `unit/test_foundations.py::test_config_validation_and_migration`, `unit/test_tasks_core.py::test_multi_workspace_isolation` |
| Config inspection/validation/migration; backup/restore; export/import; offline; schema upgrades | Done | `integration/test_cli.py::test_cli_backup_restore_export_import`, `unit/test_foundations.py::test_migrations_clean_downgrade_and_existing` |
| WAL, busy timeouts, foreign keys, safe transactions, atomic writes | Done | `unit/test_foundations.py::test_foreign_keys_enforced_and_rollback`, `::test_concurrent_writers_serialize` |
| Retention/compaction without deleting records | Done | `jobs/retention.py`, `workgraph job compact` (exercised in `integration/test_cli_groups.py`) |

## 31-33. Security, tests, end-to-end demonstration

| Requirement | Status | Evidence |
| --- | --- | --- |
| Input/path/command validation, permissions, repo access, redaction, safe spawning, limits, audit | Done | `docs/security.md`; `unit/test_foundations.py`, `integration/test_providers_and_services.py::test_repository_boundary_enforced` |
| Loopback binding, token auth, Host/Origin validation, no permissive CORS, no raw SQL/shell endpoints | Done | `integration/test_api.py::test_health_is_public_but_api_requires_token`, `::test_dns_rebinding_and_origin_protection`, `integration/test_mcp.py` (401 without token) |
| Opt-in execution; no automatic push/merge/destructive cleanup | Done | `integration/test_runs.py::test_execution_is_opt_in`, `integration/test_api.py` (403) |
| Real temporary Git repositories; deterministic agent fixtures; no paid API/Ollama/Claude required | Done | `tests/conftest.py`, `demo/scripted_agent.py`, `fixtures/` |
| Backend coverage ≥ 80% | Done | `uv run tools/check.py` enforces `--cov-fail-under=80` |
| Frontend type checks, build, component tests, browser e2e | Done | `pnpm typecheck`, `pnpm build`, `pnpm test` (13), `pnpm test:e2e` (3) |
| 25-step cross-repository demonstration, offline | Done | `workgraph demo e2e`, `e2e/test_demo.py::test_required_end_to_end_demonstration` |

## 34. Documentation

Done: `README.md` and `docs/` (architecture, data model, getting started/offline, configuration,
CLI, API, MCP, Claude Code, agent protocol, inference, Git/multi-repo, task lifecycle, automation,
analytics definitions, security, backup/recovery, extending, testing, troubleshooting).

## Known limitations

- The Claude Code adapter and Ollama provider were verified against faithful local stand-ins
  (documented `stream-json` output; an HTTP server implementing the Ollama API), not against the
  real services, to keep the suite free of credentials, quota use and model downloads.
- Verification ran on Windows only.
- Agent branches are never merged automatically (by design); follow-up tasks base their worktrees
  on the repository's default branch, so dependent agent work sees earlier agent changes only after
  a human merges them.
- The Streamable HTTP MCP endpoint path is `/mcp/` (with the trailing slash).
- The production web bundle is a single ~980 KB chunk (288 KB gzipped).
- The analytics snapshot is rebuilt in memory when data changes; very large histories (millions of
  events) would make the first query after a change slower.

## Resuming work

Architecture decisions are recorded in `docs/architecture.md`; extension points in
`docs/extending.md`. Run `uv run tools/check.py` and `uv run workgraph demo e2e` to confirm a
healthy tree before changing anything.
