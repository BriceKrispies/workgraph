//! Recursion depth and rate guards, and the status a run receives from them
//! (`AutomationService._create_run` and `_rate_limited` in `automation/engine.py`).

use serde_json::Value;

/// Automation runs allowed per rule in any one minute.
pub const RATE_LIMIT_PER_MINUTE: i64 = 30;

/// `int(value)` as Python applies it to a stored depth: numbers truncate, booleans count as
/// 0 or 1, numeric text parses, and anything else is 0.
fn py_int(value: &Value) -> i64 {
    match value {
        Value::Bool(flag) => i64::from(*flag),
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64))
            .unwrap_or(0),
        Value::String(text) => text.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// The automation depth an event carries in `data._automation.depth`, or 0.
pub fn event_depth(data: &Value) -> i64 {
    data.get("_automation")
        .filter(|meta| meta.is_object())
        .and_then(|meta| meta.get("depth"))
        .map_or(0, py_int)
}

/// The depth of a run triggered by an event at `event_depth`: one more than the event's.
pub const fn run_depth(event_depth: i64) -> i64 {
    event_depth + 1
}

/// The depth limit that applies: the smaller of the rule's and the configured maximum.
pub fn depth_limit(rule_max_depth: i64, config_max_depth: i64) -> i64 {
    rule_max_depth.min(config_max_depth)
}

/// Whether `rate` recent runs in the last minute exhaust the rule's budget.
pub const fn rate_limited(recent_runs: i64) -> bool {
    recent_runs >= RATE_LIMIT_PER_MINUTE
}

/// The outcome of one run before any row is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunDecision {
    /// Refused by a guard; the reason is the recorded text.
    Skipped(String),
    /// The conditions did not hold.
    NotMatched,
    /// A dry run whose conditions held; the actions are only described.
    Previewed,
    /// The conditions held and the actions are queued.
    Queued,
}

impl RunDecision {
    /// The `status` column value.
    pub const fn status(&self) -> &'static str {
        match self {
            RunDecision::Skipped(_) => "skipped",
            RunDecision::NotMatched => "not_matched",
            RunDecision::Previewed => "previewed",
            RunDecision::Queued => "queued",
        }
    }

    /// The `reason` column text.
    pub fn reason(&self) -> String {
        match self {
            RunDecision::Skipped(reason) => reason.clone(),
            RunDecision::NotMatched => "conditions evaluated to false".to_owned(),
            RunDecision::Previewed => "dry run: actions not executed".to_owned(),
            RunDecision::Queued => "conditions matched".to_owned(),
        }
    }
}

/// Decides a run. The depth guard applies to dry runs too; the rate guard only to real runs,
/// and `matched` is consulted only when no guard refuses the run.
pub fn decide_run(
    depth: i64,
    depth_limit: i64,
    dry_run: bool,
    rate_limited: bool,
    matched: impl FnOnce() -> bool,
) -> RunDecision {
    if depth > depth_limit {
        return RunDecision::Skipped(format!(
            "automation depth {depth} exceeds limit {depth_limit} (recursion guard)"
        ));
    }
    if !dry_run && rate_limited {
        return RunDecision::Skipped(format!(
            "rate limit of {RATE_LIMIT_PER_MINUTE} runs/minute exceeded"
        ));
    }
    if !matched() {
        RunDecision::NotMatched
    } else if dry_run {
        RunDecision::Previewed
    } else {
        RunDecision::Queued
    }
}
