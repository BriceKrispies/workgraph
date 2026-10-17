"""Metric registry: explicit definitions and safe, parameterised analytical queries.

Every metric has a written definition, a unit, a statistic kind, a base relation and
the dimensions it can be grouped by. Queries are composed only from this registry
and validated parameters - never from caller-supplied SQL.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Literal

Kind = Literal["count", "distribution", "ratio", "sum"]

# Each base is a subquery exposing ``ws`` (workspace id) and the dimension columns
# listed in BASE_DIMENSIONS, plus metric-specific value columns.
BASES: dict[str, str] = {
    "tasks": """
        SELECT tf.*, tf.workspace_id AS ws, tf.project_id AS project, tf.category AS category,
               tf.priority AS priority, tf.status AS status, tf.assigned_agent AS agent
        FROM task_facts tf WHERE tf.archived_at IS NULL
    """,
    "attempts": """
        SELECT af.*, af.workspace_id AS ws, af.project_id AS project, af.category AS category,
               af.priority AS priority, af.status AS status, af.provider AS provider, af.provider AS agent,
               COALESCE(af.model, '(unspecified)') AS model
        FROM attempt_facts af WHERE af.parent_attempt_id IS NULL
    """,
    "runs": """
        SELECT r.*, r.workspace_id AS ws, t.project_id AS project, t.category AS category, t.priority AS priority,
               r.status AS status, COALESCE(a.provider, '(manual)') AS provider, COALESCE(a.provider, '(manual)') AS agent,
               COALESCE(a.model, '(unspecified)') AS model
        FROM verification_runs r JOIN task_facts t ON t.id = r.task_id
        LEFT JOIN agent_attempts a ON a.id = r.attempt_id
    """,
    "checks": """
        SELECT c.*, r.workspace_id AS ws, t.project_id AS project, t.category AS category, c.status AS status,
               c.kind AS kind, c.name AS check_name, c.repository_id AS repository, r.task_id AS task_id
        FROM verification_check_results c JOIN verification_runs r ON r.id = c.run_id
        JOIN task_facts t ON t.id = r.task_id
    """,
    "relations": """
        SELECT rel.*, t.workspace_id AS ws, t.project_id AS project, t.category AS category,
               rel.relation_type AS relation_type
        FROM task_relations rel JOIN task_facts t ON t.id = rel.source_task_id
    """,
    "files": """
        SELECT f.*, t.workspace_id AS ws, t.project_id AS project, t.category AS category, f.module AS module,
               f.repository_id AS repository, f.file_class AS file_class, t.created_at AS created_at,
               t.accepted_at AS accepted_at
        FROM task_files f JOIN task_facts t ON t.id = f.task_id
    """,
    "repo_commits": """
        SELECT DISTINCT c.id, c.repository_id AS repository, c.insertions, c.deletions, c.committed_at,
               c.is_merge, r.workspace_id AS ws
        FROM task_commits c JOIN repositories r ON r.id = c.repository_id
    """,
}

BASE_DIMENSIONS: dict[str, set[str]] = {
    "tasks": {"project", "category", "priority", "status", "agent", "repository"},
    "attempts": {"project", "category", "priority", "status", "provider", "model", "agent"},
    "runs": {"project", "category", "status", "provider", "model", "agent"},
    "checks": {"project", "category", "status", "kind", "check_name", "repository"},
    "relations": {"project", "category", "relation_type"},
    "files": {"project", "category", "module", "repository", "file_class"},
    "repo_commits": {"repository"},
}
PERIODS = {"day", "week", "month", "quarter", "year"}


@dataclass(frozen=True)
class Metric:
    id: str
    family: str
    title: str
    definition: str
    unit: str
    kind: Kind
    base: str
    time_column: str
    value: str = "1"
    where: str = "TRUE"
    numerator: str = ""
    denominator: str = ""
    caveats: list[str] = field(default_factory=list)


M = Metric
METRICS: list[Metric] = [
    # throughput ---------------------------------------------------------------------------
    M(
        "tasks_created",
        "throughput",
        "Tasks created",
        "Number of tasks created in the period.",
        "tasks",
        "count",
        "tasks",
        "created_at",
    ),
    M(
        "tasks_completed",
        "throughput",
        "Tasks completed",
        "Tasks whose completion was reported (entered awaiting verification/review), by completion time. "
        "Completion is a claim, not acceptance.",
        "tasks",
        "count",
        "tasks",
        "completed_at",
        where="completed_at IS NOT NULL",
    ),
    M(
        "tasks_accepted",
        "throughput",
        "Tasks accepted",
        "Tasks accepted (verified or manually accepted), by acceptance time.",
        "tasks",
        "count",
        "tasks",
        "accepted_at",
        where="accepted_at IS NOT NULL",
    ),
    M(
        "open_backlog",
        "throughput",
        "Open tasks",
        "Tasks currently not accepted or cancelled, grouped by creation period.",
        "tasks",
        "count",
        "tasks",
        "created_at",
        where="status NOT IN ('accepted', 'cancelled')",
    ),
    M(
        "blocked_tasks",
        "throughput",
        "Blocked tasks",
        "Tasks currently in the blocked state.",
        "tasks",
        "count",
        "tasks",
        "created_at",
        where="status = 'blocked'",
    ),
    M(
        "reopened_tasks",
        "quality",
        "Reopened tasks",
        "Tasks reopened at least once after acceptance or cancellation.",
        "tasks",
        "count",
        "tasks",
        "created_at",
        where="reopen_count > 0",
    ),
    # execution ----------------------------------------------------------------------------
    M(
        "queue_wait",
        "execution",
        "Queue waiting time",
        "Time between an attempt being queued and starting, per top-level attempt (measured by Workgraph).",
        "ms",
        "distribution",
        "attempts",
        "started_at",
        value="queue_wait_ms",
        where="queue_wait_ms IS NOT NULL",
    ),
    M(
        "execution_duration",
        "execution",
        "Active execution time",
        "Measured wall-clock duration of finished top-level attempts.",
        "ms",
        "distribution",
        "attempts",
        "finished_at",
        value="duration_ms",
        where="duration_ms IS NOT NULL AND finished_at IS NOT NULL",
    ),
    M(
        "task_elapsed",
        "execution",
        "Total task elapsed time",
        "Creation to acceptance, for accepted tasks.",
        "ms",
        "distribution",
        "tasks",
        "accepted_at",
        value="elapsed_ms",
        where="elapsed_ms IS NOT NULL",
    ),
    M(
        "cycle_time",
        "execution",
        "Cycle time",
        "First start (claim/run) to acceptance, for accepted tasks.",
        "ms",
        "distribution",
        "tasks",
        "accepted_at",
        value="cycle_ms",
        where="cycle_ms IS NOT NULL",
    ),
    M(
        "attempts_per_task",
        "execution",
        "Attempts per task",
        "Top-level attempts per accepted task that had attempts.",
        "attempts",
        "distribution",
        "tasks",
        "accepted_at",
        value="attempts",
        where="accepted_at IS NOT NULL AND attempts > 0",
    ),
    M(
        "retry_rate",
        "execution",
        "Retry frequency",
        "Share of top-level attempts that were explicit retries.",
        "ratio",
        "ratio",
        "attempts",
        "created_at",
        numerator="COUNT(*) FILTER (WHERE retry_of_attempt_id IS NOT NULL)",
        denominator="COUNT(*)",
    ),
    M(
        "first_pass_success",
        "execution",
        "First-pass success",
        "Accepted tasks (with attempts) whose first attempt succeeded and needed no further attempts.",
        "ratio",
        "ratio",
        "tasks",
        "accepted_at",
        where="accepted_at IS NOT NULL AND attempts > 0",
        numerator="COUNT(*) FILTER (WHERE attempts = 1 AND first_attempt_status = 'succeeded')",
        denominator="COUNT(*)",
    ),
    M(
        "attempt_success_rate",
        "execution",
        "Attempt success rate",
        "Succeeded / finished top-level attempts.",
        "ratio",
        "ratio",
        "attempts",
        "finished_at",
        where="status IN ('succeeded','failed','timed_out','cancelled','abandoned')",
        numerator="COUNT(*) FILTER (WHERE status = 'succeeded')",
        denominator="COUNT(*)",
    ),
    M(
        "attempt_outcomes",
        "execution",
        "Attempt outcomes",
        "Top-level attempts by status (group by status).",
        "attempts",
        "count",
        "attempts",
        "created_at",
    ),
    # code ---------------------------------------------------------------------------------
    M(
        "cumulative_churn",
        "code",
        "Cumulative churn",
        "Lines added + removed across a task's distinct linked non-merge commits (confidence >= 0.5; cherry-picked or "
        "rewritten duplicates counted once).",
        "lines",
        "distribution",
        "tasks",
        "created_at",
        value="churn_insertions + churn_deletions",
        where="commits > 0",
        caveats=["Lines changed do not measure difficulty or necessity."],
    ),
    M(
        "final_change_size",
        "code",
        "Final change size",
        "Lines added + removed between the attempt baseline and the accepted revision (all repositories).",
        "lines",
        "distribution",
        "tasks",
        "accepted_at",
        value="final_insertions + final_deletions",
        where="final_files IS NOT NULL",
    ),
    M(
        "files_touched",
        "code",
        "Files affected",
        "Distinct file paths touched by a task's linked commits.",
        "files",
        "distribution",
        "tasks",
        "created_at",
        value="files_touched",
        where="commits > 0",
    ),
    M(
        "functions_affected",
        "code",
        "Functions affected",
        "Functions/methods touched (Tree-sitter) summed over a task's analysed commits.",
        "functions",
        "distribution",
        "tasks",
        "created_at",
        value="churn_functions",
        where="commits > 0",
    ),
    M(
        "rework_ratio",
        "code",
        "Rework indicator",
        "Cumulative churn divided by final change size; values well above 1 mean code was rewritten during the task.",
        "ratio",
        "distribution",
        "tasks",
        "accepted_at",
        value="(churn_insertions + churn_deletions)::DOUBLE / NULLIF(final_insertions + final_deletions, 0)",
        where="commits > 0 AND COALESCE(final_insertions + final_deletions, 0) > 0",
    ),
    M(
        "repeated_file_edits",
        "code",
        "Repeated edits",
        "Files edited by more than one commit within the same task.",
        "files",
        "distribution",
        "tasks",
        "created_at",
        value="repeated_file_edits",
        where="commits > 0",
    ),
    M(
        "repository_change_volume",
        "code",
        "Repository change volume",
        "Lines changed by distinct task-linked commits per repository (a commit shared by several tasks counts once).",
        "lines",
        "sum",
        "repo_commits",
        "committed_at",
        value="insertions + deletions",
        where="NOT is_merge",
    ),
    M(
        "module_hotspots",
        "architecture",
        "Module hotspots",
        "Lines changed per module across tasks (group by module).",
        "lines",
        "sum",
        "files",
        "created_at",
        value="lines",
    ),
    M(
        "structural_distribution",
        "architecture",
        "Change by file class",
        "Lines changed by file class (production/test/config/docs).",
        "lines",
        "sum",
        "files",
        "created_at",
        value="lines",
    ),
    # agents -------------------------------------------------------------------------------
    M(
        "agent_completion_rate",
        "agents",
        "Agent completion rate",
        "Succeeded / finished top-level attempts, typically grouped by provider and model.",
        "ratio",
        "ratio",
        "attempts",
        "finished_at",
        where="status IN ('succeeded','failed','timed_out','cancelled','abandoned')",
        numerator="COUNT(*) FILTER (WHERE status = 'succeeded')",
        denominator="COUNT(*)",
        caveats=[
            "Agents receive different tasks; differences are not caused by the agent alone (selection bias)."
        ],
    ),
    M(
        "agent_duration",
        "agents",
        "Median agent execution duration",
        "Duration of succeeded top-level attempts.",
        "ms",
        "distribution",
        "attempts",
        "finished_at",
        value="duration_ms",
        where="status = 'succeeded' AND duration_ms IS NOT NULL",
    ),
    M(
        "agent_verification_pass_rate",
        "agents",
        "Verification success",
        "Passed / executed verification runs attached to attempts.",
        "ratio",
        "ratio",
        "attempts",
        "finished_at",
        where="verification_runs > 0",
        numerator="SUM(verification_passed)",
        denominator="SUM(verification_runs)",
    ),
    M(
        "input_tokens",
        "agents",
        "Input tokens (measured)",
        "Provider-reported input tokens; attempts without measured usage are excluded (see sample size).",
        "tokens",
        "sum",
        "attempts",
        "finished_at",
        value="input_tokens",
        where="usage_source IS NOT NULL AND input_tokens IS NOT NULL",
    ),
    M(
        "output_tokens",
        "agents",
        "Output tokens (measured)",
        "Provider-reported output tokens.",
        "tokens",
        "sum",
        "attempts",
        "finished_at",
        value="output_tokens",
        where="usage_source IS NOT NULL AND output_tokens IS NOT NULL",
    ),
    M(
        "cost",
        "agents",
        "Cost (measured)",
        "Provider-reported cost; never estimated by Workgraph.",
        "usd",
        "sum",
        "attempts",
        "finished_at",
        value="cost_usd",
        where="cost_source IS NOT NULL AND cost_usd IS NOT NULL",
    ),
    # quality ------------------------------------------------------------------------------
    M(
        "verification_pass_rate",
        "quality",
        "Verification pass rate",
        "Passed / completed verification runs.",
        "ratio",
        "ratio",
        "runs",
        "created_at",
        where="status IN ('passed','failed')",
        numerator="COUNT(*) FILTER (WHERE status = 'passed')",
        denominator="COUNT(*)",
    ),
    M(
        "check_failures",
        "quality",
        "Check failures",
        "Failed or errored verification checks (group by kind/check).",
        "checks",
        "count",
        "checks",
        "created_at",
        where="status IN ('failed','error')",
    ),
    M(
        "test_failures",
        "quality",
        "Test failures",
        "Failed test checks.",
        "checks",
        "count",
        "checks",
        "created_at",
        where="status IN ('failed','error') AND kind = 'test'",
    ),
    M(
        "build_failures",
        "quality",
        "Build failures",
        "Failed build checks.",
        "checks",
        "count",
        "checks",
        "created_at",
        where="status IN ('failed','error') AND kind = 'build'",
    ),
    M(
        "integration_failures",
        "quality",
        "Integration failures",
        "Failed cross-repository integration checks.",
        "checks",
        "count",
        "checks",
        "created_at",
        where="status IN ('failed','error') AND kind = 'integration'",
    ),
    M(
        "regressions",
        "quality",
        "Regressions linked to previous work",
        "Tasks related to earlier work as regression_of.",
        "relations",
        "count",
        "relations",
        "created_at",
        where="relation_type = 'regression_of'",
    ),
    M(
        "follow_up_fixes",
        "quality",
        "Follow-up fixes",
        "Tasks related to earlier work as follows_up.",
        "relations",
        "count",
        "relations",
        "created_at",
        where="relation_type = 'follows_up'",
    ),
]
METRICS_BY_ID = {m.id: m for m in METRICS}
