//! One rule run planned: the guards, the conditions and the dry-run actions of
//! `AutomationService._create_run`, as the values the run row records. The rows, the job and the
//! ids are the runtime's; this module decides what the run's status, reason, trace and planned
//! actions are.

use serde_json::{Map, Value};

use crate::condition;
use crate::guard::{self, RunDecision};
use crate::rules::preview_actions;

/// What one run records.
#[derive(Debug, Clone, PartialEq)]
pub struct RunPlan {
    /// `skipped`, `not_matched`, `previewed` or `queued`.
    pub status: &'static str,
    pub matched: bool,
    pub reason: String,
    /// One entry per evaluated predicate (empty when a guard refused the run).
    pub condition_trace: Vec<Value>,
    /// The planned actions of a dry run; empty otherwise.
    pub action_results: Vec<Value>,
}

impl RunPlan {
    /// The outcome `_create_run` returns: `matched` for a queued run, `previewed` for a dry run,
    /// `skipped`, or `not_matched`.
    pub fn outcome(&self, dry_run: bool) -> &'static str {
        match self.status {
            "skipped" => "skipped",
            "queued" => "matched",
            _ if dry_run => "previewed",
            _ => "not_matched",
        }
    }
}

/// What a run is planned from.
#[derive(Debug, Clone, Copy)]
pub struct RunInput<'a> {
    /// The run's depth: the triggering event's depth plus one, or one for a trigger without event.
    pub depth: i64,
    pub rule_max_depth: i64,
    pub config_max_depth: i64,
    pub dry_run: bool,
    /// Live, non-dry runs of the rule in the last minute.
    pub recent_runs: i64,
    /// The rule's stored conditions (`{}` when it has none).
    pub conditions: &'a Map<String, Value>,
    /// The rule's stored actions.
    pub actions: &'a [Map<String, Value>],
    /// The condition context (`_context`).
    pub context: &'a Value,
}

/// Plans one run. The error is the text of a stored condition that no longer validates.
pub fn plan_run(input: &RunInput<'_>) -> Result<RunPlan, String> {
    let limit = guard::depth_limit(input.rule_max_depth, input.config_max_depth);
    let guarded = guard::decide_run(
        input.depth,
        limit,
        input.dry_run,
        guard::rate_limited(input.recent_runs),
        || true,
    );
    match guarded {
        RunDecision::Skipped(reason) => Ok(RunPlan {
            status: "skipped",
            matched: false,
            reason,
            condition_trace: Vec::new(),
            action_results: Vec::new(),
        }),
        _ => evaluate(input),
    }
}

/// The guards passed: evaluate the conditions and decide between not matched, preview and queue.
fn evaluate(input: &RunInput<'_>) -> Result<RunPlan, String> {
    let condition = if input.conditions.is_empty() {
        None
    } else {
        Some(
            condition::validate(&Value::Object(input.conditions.clone()))
                .map_err(|error| condition_message(&error))?,
        )
    };
    let (matched, trace) = condition::evaluate(condition.as_ref(), input.context);
    let condition_trace: Vec<Value> = trace.iter().map(condition::TraceEntry::to_json).collect();
    let plan = if !matched {
        RunPlan {
            status: "not_matched",
            matched: false,
            reason: RunDecision::NotMatched.reason(),
            condition_trace,
            action_results: Vec::new(),
        }
    } else if input.dry_run {
        RunPlan {
            status: "previewed",
            matched: true,
            reason: RunDecision::Previewed.reason(),
            condition_trace,
            action_results: preview_actions(input.actions, input.context),
        }
    } else {
        RunPlan {
            status: "queued",
            matched: true,
            reason: RunDecision::Queued.reason(),
            condition_trace,
            action_results: Vec::new(),
        }
    };
    Ok(plan)
}

fn condition_message(error: &condition::ConditionError) -> String {
    match error {
        condition::ConditionError::Invalid(text) | condition::ConditionError::BadPattern(text) => {
            text.clone()
        }
    }
}
