//! `attempt_facts` of `analytics/warehouse.py` `_build`: one row per agent attempt whose task is in
//! the warehouse, with the task's key and classification, the token counts read from the usage
//! object, the verification runs of the attempt and the task's primary project.

use crate::agg::{Groups, aggregate, count, lookup};
use crate::source::{AnalyticsInput, AttemptRow, TaskRow};
use crate::tasks::Projects;

/// A row of `attempt_facts`.
#[derive(Debug, Clone, PartialEq)]
pub struct AttemptFact {
    pub attempt: AttemptRow,
    pub task_key: Option<String>,
    pub category: Option<String>,
    pub priority: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub verification_runs: i64,
    pub verification_passed: i64,
    pub project_id: Option<String>,
}

/// The verification runs of one attempt.
#[derive(Debug, Clone, Copy, Default)]
struct Verified {
    runs: i64,
    passed: i64,
}

/// `TRY_CAST(json_extract(usage, '$.<key>') AS BIGINT)`: NULL unless the key holds an integer
/// (or a number or string that converts to one).
fn usage_count(usage: Option<&str>, key: &str) -> Option<i64> {
    let value: serde_json::Value = serde_json::from_str(usage?).ok()?;
    match value.get(key)? {
        serde_json::Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f.round() as i64)),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// `attempt_facts`. Attempts whose task is not in `tasks` are dropped (an inner join).
pub fn attempt_facts(
    input: &AnalyticsInput,
    projects: &Groups<Option<String>, Projects>,
) -> Vec<AttemptFact> {
    let tasks: Groups<Option<String>, TaskRow> = aggregate(
        input.tasks.iter().cloned(),
        |task| task.id.clone(),
        |task| task.clone(),
        |first, _| first,
    );
    let verified: Groups<Option<String>, Verified> = aggregate(
        input.verification_runs.iter(),
        |run| run.attempt_id.clone(),
        |run| Verified {
            runs: 1,
            passed: count(run.status.as_deref() == Some("passed")),
        },
        |a, b| Verified {
            runs: a.runs + b.runs,
            passed: a.passed + b.passed,
        },
    );
    input
        .attempts
        .iter()
        .filter_map(|attempt| lookup(&tasks, &attempt.task_id).map(|task| (attempt, task)))
        .map(|(attempt, task)| {
            let runs = lookup(&verified, &attempt.id).copied().unwrap_or_default();
            AttemptFact {
                attempt: attempt.clone(),
                task_key: task.key.clone(),
                category: task.category.clone(),
                priority: task.priority.clone(),
                input_tokens: usage_count(attempt.usage.as_deref(), "input_tokens"),
                output_tokens: usage_count(attempt.usage.as_deref(), "output_tokens"),
                verification_runs: runs.runs,
                verification_passed: runs.passed,
                project_id: lookup(projects, &attempt.task_id).and_then(|p| p.first.clone()),
            }
        })
        .collect()
}
