//! Pure: the DuckDB text of the Python `AnalyticsService.build_sql` (`analytics/service.py`), so
//! the provenance `sql` of a query is the text Python reports. The Rust engine does not run this
//! text; it only reproduces it, from the same base relations and metric clauses as
//! `analytics/metrics.py`.

use wg_analytics::core::bases::Base;
use wg_analytics::core::metrics::{Metric, Statistic};
use wg_analytics::core::query::Query;

/// The base relation text (`BASES[metric.base]`).
fn base_sql(base: Base) -> &'static str {
    match base {
        Base::Tasks => {
            "\n        SELECT tf.*, tf.workspace_id AS ws, tf.project_id AS project, tf.category AS category,\n               tf.priority AS priority, tf.status AS status, tf.assigned_agent AS agent\n        FROM task_facts tf WHERE tf.archived_at IS NULL\n    "
        }
        Base::Attempts => {
            "\n        SELECT af.*, af.workspace_id AS ws, af.project_id AS project, af.category AS category,\n               af.priority AS priority, af.status AS status, af.provider AS provider, af.provider AS agent,\n               COALESCE(af.model, '(unspecified)') AS model\n        FROM attempt_facts af WHERE af.parent_attempt_id IS NULL\n    "
        }
        Base::Runs => {
            "\n        SELECT r.*, r.workspace_id AS ws, t.project_id AS project, t.category AS category, t.priority AS priority,\n               r.status AS status, COALESCE(a.provider, '(manual)') AS provider, COALESCE(a.provider, '(manual)') AS agent,\n               COALESCE(a.model, '(unspecified)') AS model\n        FROM verification_runs r JOIN task_facts t ON t.id = r.task_id\n        LEFT JOIN agent_attempts a ON a.id = r.attempt_id\n    "
        }
        Base::Checks => {
            "\n        SELECT c.*, r.workspace_id AS ws, t.project_id AS project, t.category AS category, c.status AS status,\n               c.kind AS kind, c.name AS check_name, c.repository_id AS repository, r.task_id AS task_id\n        FROM verification_check_results c JOIN verification_runs r ON r.id = c.run_id\n        JOIN task_facts t ON t.id = r.task_id\n    "
        }
        Base::Relations => {
            "\n        SELECT rel.*, t.workspace_id AS ws, t.project_id AS project, t.category AS category,\n               rel.relation_type AS relation_type\n        FROM task_relations rel JOIN task_facts t ON t.id = rel.source_task_id\n    "
        }
        Base::Files => {
            "\n        SELECT f.*, t.workspace_id AS ws, t.project_id AS project, t.category AS category, f.module AS module,\n               f.repository_id AS repository, f.file_class AS file_class, t.created_at AS created_at,\n               t.accepted_at AS accepted_at\n        FROM task_files f JOIN task_facts t ON t.id = f.task_id\n    "
        }
        Base::RepoCommits => {
            "\n        SELECT DISTINCT c.id, c.repository_id AS repository, c.insertions, c.deletions, c.committed_at,\n               c.is_merge, r.workspace_id AS ws\n        FROM task_commits c JOIN repositories r ON r.id = c.repository_id\n    "
        }
    }
}

/// The metric clauses of `analytics/metrics.py`: `where`, `value`, `numerator`, `denominator`.
fn clauses(id: &str) -> (&'static str, &'static str, &'static str, &'static str) {
    const TABLE: &[(&str, &str, &str, &str, &str)] = &[
        ("tasks_created", "TRUE", "1", "", ""),
        ("tasks_completed", "completed_at IS NOT NULL", "1", "", ""),
        ("tasks_accepted", "accepted_at IS NOT NULL", "1", "", ""),
        (
            "open_backlog",
            "status NOT IN ('accepted', 'cancelled')",
            "1",
            "",
            "",
        ),
        ("blocked_tasks", "status = 'blocked'", "1", "", ""),
        ("reopened_tasks", "reopen_count > 0", "1", "", ""),
        (
            "queue_wait",
            "queue_wait_ms IS NOT NULL",
            "queue_wait_ms",
            "",
            "",
        ),
        (
            "execution_duration",
            "duration_ms IS NOT NULL AND finished_at IS NOT NULL",
            "duration_ms",
            "",
            "",
        ),
        (
            "task_elapsed",
            "elapsed_ms IS NOT NULL",
            "elapsed_ms",
            "",
            "",
        ),
        ("cycle_time", "cycle_ms IS NOT NULL", "cycle_ms", "", ""),
        (
            "attempts_per_task",
            "accepted_at IS NOT NULL AND attempts > 0",
            "attempts",
            "",
            "",
        ),
        (
            "retry_rate",
            "TRUE",
            "1",
            "COUNT(*) FILTER (WHERE retry_of_attempt_id IS NOT NULL)",
            "COUNT(*)",
        ),
        (
            "first_pass_success",
            "accepted_at IS NOT NULL AND attempts > 0",
            "1",
            "COUNT(*) FILTER (WHERE attempts = 1 AND first_attempt_status = 'succeeded')",
            "COUNT(*)",
        ),
        (
            "attempt_success_rate",
            "status IN ('succeeded','failed','timed_out','cancelled','abandoned')",
            "1",
            "COUNT(*) FILTER (WHERE status = 'succeeded')",
            "COUNT(*)",
        ),
        ("attempt_outcomes", "TRUE", "1", "", ""),
        (
            "cumulative_churn",
            "commits > 0",
            "churn_insertions + churn_deletions",
            "",
            "",
        ),
        (
            "final_change_size",
            "final_files IS NOT NULL",
            "final_insertions + final_deletions",
            "",
            "",
        ),
        ("files_touched", "commits > 0", "files_touched", "", ""),
        (
            "functions_affected",
            "commits > 0",
            "churn_functions",
            "",
            "",
        ),
        (
            "rework_ratio",
            "commits > 0 AND COALESCE(final_insertions + final_deletions, 0) > 0",
            "(churn_insertions + churn_deletions)::DOUBLE / NULLIF(final_insertions + final_deletions, 0)",
            "",
            "",
        ),
        (
            "repeated_file_edits",
            "commits > 0",
            "repeated_file_edits",
            "",
            "",
        ),
        (
            "repository_change_volume",
            "NOT is_merge",
            "insertions + deletions",
            "",
            "",
        ),
        ("module_hotspots", "TRUE", "lines", "", ""),
        ("structural_distribution", "TRUE", "lines", "", ""),
        (
            "agent_completion_rate",
            "status IN ('succeeded','failed','timed_out','cancelled','abandoned')",
            "1",
            "COUNT(*) FILTER (WHERE status = 'succeeded')",
            "COUNT(*)",
        ),
        (
            "agent_duration",
            "status = 'succeeded' AND duration_ms IS NOT NULL",
            "duration_ms",
            "",
            "",
        ),
        (
            "agent_verification_pass_rate",
            "verification_runs > 0",
            "1",
            "SUM(verification_passed)",
            "SUM(verification_runs)",
        ),
        (
            "input_tokens",
            "usage_source IS NOT NULL AND input_tokens IS NOT NULL",
            "input_tokens",
            "",
            "",
        ),
        (
            "output_tokens",
            "usage_source IS NOT NULL AND output_tokens IS NOT NULL",
            "output_tokens",
            "",
            "",
        ),
        (
            "cost",
            "cost_source IS NOT NULL AND cost_usd IS NOT NULL",
            "cost_usd",
            "",
            "",
        ),
        (
            "verification_pass_rate",
            "status IN ('passed','failed')",
            "1",
            "COUNT(*) FILTER (WHERE status = 'passed')",
            "COUNT(*)",
        ),
        (
            "check_failures",
            "status IN ('failed','error')",
            "1",
            "",
            "",
        ),
        (
            "test_failures",
            "status IN ('failed','error') AND kind = 'test'",
            "1",
            "",
            "",
        ),
        (
            "build_failures",
            "status IN ('failed','error') AND kind = 'build'",
            "1",
            "",
            "",
        ),
        (
            "integration_failures",
            "status IN ('failed','error') AND kind = 'integration'",
            "1",
            "",
            "",
        ),
        (
            "regressions",
            "relation_type = 'regression_of'",
            "1",
            "",
            "",
        ),
        (
            "follow_up_fixes",
            "relation_type = 'follows_up'",
            "1",
            "",
            "",
        ),
    ];
    TABLE
        .iter()
        .find(|entry| entry.0 == id)
        .map_or(("TRUE", "1", "", ""), |entry| {
            (entry.1, entry.2, entry.3, entry.4)
        })
}

/// `build_sql`'s text for a query that passed validation: the same clauses in the same order.
#[must_use]
pub fn build(metric: &Metric, query: &Query) -> String {
    let (where_clause, value, numerator, denominator) = clauses(metric.id);
    let time = metric.time.name();
    let filters = &query.filters;
    let grouped_by_repository = query.group_by.iter().any(|dim| dim == "repository");
    let mut base = base_sql(metric.base).to_owned();
    if metric.base == Base::Tasks && grouped_by_repository {
        base = format!(
            "SELECT b.*, tr.repository_id AS repository FROM ({base}) b JOIN task_repositories tr ON tr.task_id = b.id"
        );
    }
    let mut conditions = vec!["ws = ?".to_owned(), format!("({where_clause})")];
    if filters.since.is_some() {
        conditions.push(format!("{time} >= ?"));
    }
    if filters.until.is_some() {
        conditions.push(format!("{time} < ?"));
    }
    let simple = [
        ("project", filters.project.is_some()),
        ("category", filters.category.is_some()),
        ("priority", filters.priority.is_some()),
        ("status", filters.status.is_some()),
        ("model", filters.model.is_some()),
        ("kind", filters.kind.is_some()),
        ("provider", filters.provider.is_some()),
        ("agent", filters.agent.is_some()),
    ];
    for (column, present) in simple {
        if present {
            conditions.push(format!("{column} = ?"));
        }
    }
    if filters.repository.is_some() {
        if metric.base == Base::Tasks && !grouped_by_repository {
            conditions.push(
                "EXISTS (SELECT 1 FROM task_repositories tr2 WHERE tr2.task_id = id AND tr2.repository_id = ?)"
                    .to_owned(),
            );
        } else {
            conditions.push("repository = ?".to_owned());
        }
    }
    let period = query.period.name();
    let mut select: Vec<String> = query
        .group_by
        .iter()
        .map(|dim| {
            if dim == "period" {
                format!("date_trunc('{period}', {time}) AS period")
            } else {
                dim.clone()
            }
        })
        .collect();
    select.push(match metric.statistic {
        Statistic::Count => "COUNT(*) AS value, COUNT(*) AS n".to_owned(),
        Statistic::Sum => format!("SUM({value}) AS value, COUNT(*) AS n"),
        Statistic::Ratio => format!(
            "{numerator} AS numerator, {denominator} AS denominator, ({numerator})::DOUBLE / NULLIF({denominator}, 0) AS value, COUNT(*) AS n"
        ),
        Statistic::Distribution => format!(
            "COUNT({value}) AS n, AVG({value}) AS mean, quantile_cont({value}, 0.5) AS median, quantile_cont({value}, 0.75) AS p75, quantile_cont({value}, 0.9) AS p90, quantile_cont({value}, 0.95) AS p95, MIN({value}) AS min, MAX({value}) AS max, SUM({value}) AS sum"
        ),
    });
    let mut sql = format!(
        "SELECT {} FROM ({base}) WHERE {}",
        select.join(", "),
        conditions.join(" AND ")
    );
    if !query.group_by.is_empty() {
        let columns = query.group_by.join(", ");
        sql.push_str(&format!(" GROUP BY {columns} ORDER BY {columns}"));
    }
    sql.push_str(&format!(" LIMIT {}", query.limit));
    sql
}
