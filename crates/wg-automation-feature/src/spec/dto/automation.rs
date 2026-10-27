//! The automation outputs: `RuleOut` and `RunOut`, the catalog, and the `automation_list` MCP
//! output (`{"rules", "recent_runs"}`).

use wg_json::{Value, dict};
use wg_model::entity::automation::{AutomationRule, AutomationRun};
use wg_types::Id;

use wg_wire::convention::{
    TimeFormat, id, json_object, model, optional_timestamp, text, timestamp,
};

/// The Python class of a rule (`RuleOut`); not an OpenAPI component schema.
pub const RULE_NAME: &str = "RuleOut";

/// The rule fields in Python declaration order.
pub const RULE_FIELDS: [&str; 16] = [
    "id",
    "workspace_id",
    "name",
    "description",
    "enabled",
    "trigger",
    "conditions",
    "actions",
    "retry_max_attempts",
    "max_depth",
    "last_scheduled_at",
    "next_run_at",
    "created_by",
    "version",
    "created_at",
    "updated_at",
];

/// The Python class of a run (`RunOut`); not an OpenAPI component schema.
pub const RUN_NAME: &str = "RunOut";

/// The run fields in Python declaration order.
pub const RUN_FIELDS: [&str; 17] = [
    "id",
    "rule_id",
    "workspace_id",
    "trigger_type",
    "trigger_key",
    "trigger_event_id",
    "depth",
    "dry_run",
    "status",
    "matched",
    "reason",
    "condition_trace",
    "action_results",
    "error",
    "attempts",
    "created_at",
    "finished_at",
];

/// The field names of the `automation_list` MCP output dict.
pub const LIST_FIELDS: [&str; 2] = ["rules", "recent_runs"];

/// The catalog's fields, in Python order.
pub const CATALOG_FIELDS: [&str; 5] = ["triggers", "schedule", "manual", "actions", "operators"];

/// A rule as the API and the MCP tools return it (`RuleOut`).
pub fn rule(rule: &AutomationRule) -> Value {
    model(
        RULE_NAME,
        vec![
            ("id", id(rule.id)),
            ("workspace_id", id(rule.workspace_id)),
            ("name", text(&rule.name)),
            ("description", text(&rule.description)),
            ("enabled", Value::Bool(rule.enabled)),
            ("trigger", json_object(&rule.trigger)),
            ("conditions", json_object(&rule.conditions)),
            (
                "actions",
                Value::List(rule.actions.iter().map(json_object).collect()),
            ),
            (
                "retry_max_attempts",
                Value::Int(i128::from(rule.retry_max_attempts)),
            ),
            ("max_depth", Value::Int(i128::from(rule.max_depth))),
            (
                "last_scheduled_at",
                optional_timestamp(rule.last_scheduled_at, TimeFormat::ModelZ),
            ),
            (
                "next_run_at",
                optional_timestamp(rule.next_run_at, TimeFormat::ModelZ),
            ),
            ("created_by", text(&rule.created_by)),
            ("version", Value::Int(i128::from(rule.version))),
            ("created_at", timestamp(rule.created_at, TimeFormat::ModelZ)),
            ("updated_at", timestamp(rule.updated_at, TimeFormat::ModelZ)),
        ],
    )
}

/// A run of a rule, as a trigger, a preview or a run listing returns it (`RunOut`).
pub fn run(run: &AutomationRun) -> Value {
    model(
        RUN_NAME,
        vec![
            ("id", id(run.id)),
            ("rule_id", id(run.rule_id)),
            ("workspace_id", id(run.workspace_id)),
            ("trigger_type", text(&run.trigger_type)),
            ("trigger_key", text(&run.trigger_key)),
            ("trigger_event_id", optional_id(run.trigger_event_id)),
            ("depth", Value::Int(i128::from(run.depth))),
            ("dry_run", Value::Bool(run.dry_run)),
            ("status", text(&run.status)),
            ("matched", Value::Bool(run.matched)),
            ("reason", text(&run.reason)),
            (
                "condition_trace",
                Value::List(run.condition_trace.iter().map(json_object).collect()),
            ),
            (
                "action_results",
                Value::List(run.action_results.iter().map(json_object).collect()),
            ),
            ("error", run.error.as_deref().map_or(Value::Null, text)),
            ("attempts", Value::Int(i128::from(run.attempts))),
            ("created_at", timestamp(run.created_at, TimeFormat::ModelZ)),
            (
                "finished_at",
                optional_timestamp(run.finished_at, TimeFormat::ModelZ),
            ),
        ],
    )
}

/// The catalog of triggers, actions and condition operators (`GET /automation/catalog`). The
/// schedule keys and the manual flag are fixed in the Python catalog; the triggers, actions and
/// operators are passed in, in the order the caller wants them printed.
pub fn catalog<'a>(
    triggers: impl IntoIterator<Item = (&'a str, &'a str)>,
    actions: &[&str],
    operators: &[&str],
) -> Value {
    dict!({
        "triggers": dict(
            triggers
                .into_iter()
                .map(|(key, label)| (key, text(label)))
                .collect(),
        ),
        "schedule": strings(&["cron", "interval_seconds"]),
        "manual": Value::Bool(true),
        "actions": strings(actions),
        "operators": strings(operators),
    })
}

/// The `automation_list` MCP output: the rules and the recent runs, already converted.
pub fn list(rules: Vec<Value>, recent_runs: Vec<Value>) -> Value {
    dict!({
        "rules": Value::List(rules),
        "recent_runs": Value::List(recent_runs),
    })
}

/// An optional id field: `null` when absent.
fn optional_id(value: Option<Id>) -> Value {
    value.map_or(Value::Null, id)
}

fn strings(items: &[&str]) -> Value {
    Value::List(items.iter().map(|item| text(item)).collect())
}
