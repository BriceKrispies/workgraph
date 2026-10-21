# Analytics

Analytics run in DuckDB over a read-only snapshot of the SQLite database (see
[architecture.md](architecture.md#analytics)). SQLite stays authoritative; analytics never write
operational state and need no network access.

## Querying

```sh
uv run workgraph analytics metrics
uv run workgraph analytics query tasks_accepted --by period --period week
uv run workgraph analytics query execution_duration --by provider --by model --since 2026-09-01
uv run workgraph analytics query cumulative_churn --by category --repo auth-service
uv run workgraph analytics compare tasks_accepted 2026-09-01 2026-09-15 2026-09-15 2026-10-01 --by category
uv run workgraph analytics export facts.parquet --format parquet --dataset task_facts
```

HTTP: `POST /api/v1/ws/{ws}/analytics/query` with `{"metric": ..., "group_by": [...], "period": ..., "filters": {...}}`.
MCP: `analytics_query` / `analytics_report`. Only registered metric ids, allowlisted dimensions
(`period`, `project`, `repository`, `category`, `priority`, `status`, `provider`, `model`, `agent`,
`kind`, `check_name`, `module`, `file_class`, `relation_type`) and typed filters are accepted.

Every result includes the metric definition, the rows, per-group sample sizes (`n`), the generated
SQL (provenance) and notes. Distributions report `n`, mean, median, p75, p90, p95, min, max and
sum - never only an average. Ratios report numerator and denominator.

## Metric reference

<!-- metrics:start -->

| Metric | Family | Statistic | Unit | Time column | Group by | Definition |
| --- | --- | --- | --- | --- | --- | --- |
| `tasks_created` | throughput | count | tasks | `created_at` | agent, category, period, priority, project, repository, status | Number of tasks created in the period. |
| `tasks_completed` | throughput | count | tasks | `completed_at` | agent, category, period, priority, project, repository, status | Tasks whose completion was reported (entered awaiting verification/review), by completion time. Completion is a claim, not acceptance. |
| `tasks_accepted` | throughput | count | tasks | `accepted_at` | agent, category, period, priority, project, repository, status | Tasks accepted (verified or manually accepted), by acceptance time. |
| `open_backlog` | throughput | count | tasks | `created_at` | agent, category, period, priority, project, repository, status | Tasks currently not accepted or cancelled, grouped by creation period. |
| `blocked_tasks` | throughput | count | tasks | `created_at` | agent, category, period, priority, project, repository, status | Tasks currently in the blocked state. |
| `reopened_tasks` | quality | count | tasks | `created_at` | agent, category, period, priority, project, repository, status | Tasks reopened at least once after acceptance or cancellation. |
| `queue_wait` | execution | distribution | ms | `started_at` | agent, category, model, period, priority, project, provider, status | Time between an attempt being queued and starting, per top-level attempt (measured by Workgraph). |
| `execution_duration` | execution | distribution | ms | `finished_at` | agent, category, model, period, priority, project, provider, status | Measured wall-clock duration of finished top-level attempts. |
| `task_elapsed` | execution | distribution | ms | `accepted_at` | agent, category, period, priority, project, repository, status | Creation to acceptance, for accepted tasks. |
| `cycle_time` | execution | distribution | ms | `accepted_at` | agent, category, period, priority, project, repository, status | First start (claim/run) to acceptance, for accepted tasks. |
| `attempts_per_task` | execution | distribution | attempts | `accepted_at` | agent, category, period, priority, project, repository, status | Top-level attempts per accepted task that had attempts. |
| `retry_rate` | execution | ratio | ratio | `created_at` | agent, category, model, period, priority, project, provider, status | Share of top-level attempts that were explicit retries. |
| `first_pass_success` | execution | ratio | ratio | `accepted_at` | agent, category, period, priority, project, repository, status | Accepted tasks (with attempts) whose first attempt succeeded and needed no further attempts. |
| `attempt_success_rate` | execution | ratio | ratio | `finished_at` | agent, category, model, period, priority, project, provider, status | Succeeded / finished top-level attempts. |
| `attempt_outcomes` | execution | count | attempts | `created_at` | agent, category, model, period, priority, project, provider, status | Top-level attempts by status (group by status). |
| `cumulative_churn` | code | distribution | lines | `created_at` | agent, category, period, priority, project, repository, status | Lines added + removed across a task's distinct linked non-merge commits (confidence >= 0.5; cherry-picked or rewritten duplicates counted once). *Lines changed do not measure difficulty or necessity.* |
| `final_change_size` | code | distribution | lines | `accepted_at` | agent, category, period, priority, project, repository, status | Lines added + removed between the attempt baseline and the accepted revision (all repositories). |
| `files_touched` | code | distribution | files | `created_at` | agent, category, period, priority, project, repository, status | Distinct file paths touched by a task's linked commits. |
| `functions_affected` | code | distribution | functions | `created_at` | agent, category, period, priority, project, repository, status | Functions/methods touched (Tree-sitter) summed over a task's analysed commits. |
| `rework_ratio` | code | distribution | ratio | `accepted_at` | agent, category, period, priority, project, repository, status | Cumulative churn divided by final change size; values well above 1 mean code was rewritten during the task. |
| `repeated_file_edits` | code | distribution | files | `created_at` | agent, category, period, priority, project, repository, status | Files edited by more than one commit within the same task. |
| `repository_change_volume` | code | sum | lines | `committed_at` | period, repository | Lines changed by distinct task-linked commits per repository (a commit shared by several tasks counts once). |
| `module_hotspots` | architecture | sum | lines | `created_at` | category, file_class, module, period, project, repository | Lines changed per module across tasks (group by module). |
| `structural_distribution` | architecture | sum | lines | `created_at` | category, file_class, module, period, project, repository | Lines changed by file class (production/test/config/docs). |
| `agent_completion_rate` | agents | ratio | ratio | `finished_at` | agent, category, model, period, priority, project, provider, status | Succeeded / finished top-level attempts, typically grouped by provider and model. *Agents receive different tasks; differences are not caused by the agent alone (selection bias).* |
| `agent_duration` | agents | distribution | ms | `finished_at` | agent, category, model, period, priority, project, provider, status | Duration of succeeded top-level attempts. |
| `agent_verification_pass_rate` | agents | ratio | ratio | `finished_at` | agent, category, model, period, priority, project, provider, status | Passed / executed verification runs attached to attempts. |
| `input_tokens` | agents | sum | tokens | `finished_at` | agent, category, model, period, priority, project, provider, status | Provider-reported input tokens; attempts without measured usage are excluded (see sample size). |
| `output_tokens` | agents | sum | tokens | `finished_at` | agent, category, model, period, priority, project, provider, status | Provider-reported output tokens. |
| `cost` | agents | sum | usd | `finished_at` | agent, category, model, period, priority, project, provider, status | Provider-reported cost; never estimated by Workgraph. |
| `verification_pass_rate` | quality | ratio | ratio | `created_at` | agent, category, model, period, project, provider, status | Passed / completed verification runs. |
| `check_failures` | quality | count | checks | `created_at` | category, check_name, kind, period, project, repository, status | Failed or errored verification checks (group by kind/check). |
| `test_failures` | quality | count | checks | `created_at` | category, check_name, kind, period, project, repository, status | Failed test checks. |
| `build_failures` | quality | count | checks | `created_at` | category, check_name, kind, period, project, repository, status | Failed build checks. |
| `integration_failures` | quality | count | checks | `created_at` | category, check_name, kind, period, project, repository, status | Failed cross-repository integration checks. |
| `regressions` | quality | count | relations | `created_at` | category, period, project, relation_type | Tasks related to earlier work as regression_of. |
| `follow_up_fixes` | quality | count | relations | `created_at` | category, period, project, relation_type | Tasks related to earlier work as follows_up. |

<!-- metrics:end -->

Regenerate this table with `uv run tools/gen_metric_docs.py`.

## Fact tables

| Table | Grain | Notable columns |
| --- | --- | --- |
| `task_facts` | task | lifecycle timestamps, `elapsed_ms`, `cycle_ms`, attempts, retries, first-attempt status, `reopen_count`, churn (insertions/deletions/functions), files touched, repeated edits, final change surface |
| `attempt_facts` | attempt | provider, model, status, durations, measured tokens/cost with source, verification runs passed |
| `task_commits` | task x commit | distinct linked commits (confidence ≥ 0.5), patch-id de-duplicated per repository |
| `task_files` | task x file | edits and lines per path, module, file class, repository |

## Coupling analysis

`workgraph analytics coupling` reports, per repository pair: co-change count (tasks touching both),
Jaccard index, whether a dependency is **declared** (`repo depend`) and/or **observed**, and the
supporting tasks with their commit SHAs per repository. It also reports declared-but-never-observed
dependencies, the co-change matrix, cross-repository task counts, average repositories per task,
median elapsed time for cross- versus single-repository tasks, the integration-check failure rate,
modules with recurring cross-repository impact and coupling candidates (repeated co-change without
a declared dependency). These are evidence-backed observations, not architectural verdicts.

## Historical estimation

`workgraph analytics estimate WG-42` scores accepted tasks by classification, semantic or lexical
similarity, repository overlap and module overlap, keeps the best matches above a threshold and
reports percentile distributions (execution, queue, elapsed, attempts, files, churn, final change),
verification pass rate and which agents/models were used on similar tasks - listing every task used
and why. With fewer than 3 comparable tasks it returns `insufficient_data` instead of a number.
Agent recommendations are subject to selection bias and are labelled as such.

## Failure patterns

`workgraph analytics failures` deterministically detects frequently failing tests (from parsed
failure signatures), common failure categories, failing checks, retry loops, modules with high
rework, repositories with high integration failure rates, dependency bottlenecks and work followed
by regression/follow-up fixes. Every finding links to tasks, attempts, tests and commits.
`--persist` stores findings with first/last detection times.

## Retrospectives

`workgraph analytics retrospective --days 14` builds a deterministic Markdown report (work completed,
category breakdown, bottlenecks, rework, agent effectiveness, cross-repository coordination, quality,
significant changes, opportunities) with task-key citations. If the summarization model is available
it adds a clearly labelled model-generated narrative written only from the measured facts; otherwise
the deterministic report stands alone.

## Subagent work over time

`workgraph analytics subagents [--period week|month] [--since YYYY-MM-DD] [--until YYYY-MM-DD] [--model M]`
(MCP: `analytics_subagents`) reports the Claude Code subagent runs recorded for a workspace, bucketed by
ISO week or calendar month (UTC) of when each run stopped, and by model. The model is the measured model
of the run, else its requested model, else `unknown`.

A run is *linked* when its subagent prompt starts with a `Task-ID: <KEY>` line, so it names a task in the
workspace. Runs without a link have no workspace; they are counted only when the default workspace is
reported, and the report says how many there are.

Columns, per period and model:

- `runs` and `linked_runs`: runs that stopped in the bucket; those linked to a task.
- `tasks`: distinct linked tasks. A task with runs in several buckets is counted in the bucket of its latest run.
- `accepted`, `rejected`, `failed`, `pending`: the task outcome. `accepted` is the task status `accepted`;
  `rejected` is any acceptance decision other than `accepted` on a task that is not accepted; `failed` is
  status `failed` or `cancelled`; everything else is `pending`. Decided is accepted + rejected + failed.
- `rate` (`acceptance_rate`): accepted / decided, with its 95% Wilson interval (`wilson`). Null when nothing is decided.
- `first_pass`: accepted tasks that had exactly one linked run of that model in the window.
- `median_output_tokens`, `median_turns`, `tool_errors`: measured from each run's transcript telemetry.

Task size comes from the task's final change surface (insertions + deletions, files changed) when one exists.
Otherwise it is the sum over its linked non-merge commits, each counted once. Size bands are
`floor(log2(lines))`, so band `2^b` holds tasks with `2^b` to `2^(b+1) - 1` changed lines. A task with no
change surface and no linked commits, or with zero changed lines, is in the `unknown` band.

To link runs, start the subagent prompt with `Task-ID: <KEY>` as its first line.

## Where subagent time goes

`workgraph analytics time [--since] [--until] [--by class|model|task|description] [--model M]`
(MCP: `analytics_agent_time`) splits the measured wall time of subagent runs. `analytics bottlenecks [--since]
[--until] [--top N]` (MCP: `analytics_bottlenecks`) adds the slowest calls, the number of subagent runs live
when each one ran, and agent wall time per task. The web Analytics page shows the same under "Agents & models".

The breakdown is computed from each run's transcript when it is read (`integrations/claude_code/timing.py`).
Every tool call is paired with its result by id, so its duration is measured. Model time is the gap before each
assistant message. Waiting is the gap from a turn that ended without a tool call to the notification that
resumed the agent. Calls are classified by the `TOOL_CLASSES` table, which is data: a shell command takes the
first class whose pattern matches it. Shares are of accounted time (tool + model + waiting), so they sum to 1.

Concurrency is machine-wide: it counts every stopped subagent run, in any workspace, whose interval contains
the call's start. Runs recorded before this breakdown existed have none; `workgraph claude backfill-time`
computes it from their transcripts. It is idempotent and writes only `telemetry.time`.

## Measurements versus interpretations

| Measured (facts) | Interpreted (model output) |
| --- | --- |
| Git statistics, structural symbol counts, timings recorded by Workgraph, check exit codes, provider-reported tokens/cost | classifications, decomposition proposals, narrative text |

Metrics never use model output except as a grouping dimension (`category`), which is itself
labelled as an inference with its provenance. Correlation between dimensions and outcomes is
not causation; results note this.
