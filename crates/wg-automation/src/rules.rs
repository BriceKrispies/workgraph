//! The rule catalog, trigger validation and dry-run previews (`automation/rules.py` and
//! `AutomationService.catalog`, `_describe`, `preview`).

use serde_json::{Map, Value, json};
use wg_types::WgError;

use crate::condition::OPERATORS;
use crate::cron::Cron;
use crate::py::{str_of, truthy};
use crate::template::render_default;

/// Event types that rules may subscribe to, with their labels, in catalog order.
pub const TRIGGER_EVENTS: [(&str, &str); 11] = [
    ("task.created", "Task created"),
    ("task.updated", "Task updated"),
    ("task.became_ready", "Task becomes ready"),
    ("task.completed", "Task completed"),
    ("attempt.failed", "Attempt failed"),
    ("verification.completed", "Verification completed"),
    ("git.commit_ingested", "Git commit ingested"),
    ("classification.completed", "Classification completed"),
    ("repository.changes_detected", "Repository changes detected"),
    ("task.accepted", "Task accepted"),
    ("task.status_changed", "Task status changed"),
];

/// Action types, sorted as the catalog lists them.
pub const ACTION_TYPES: [&str; 9] = [
    "add_tags",
    "classify_task",
    "create_child_tasks",
    "create_dependency",
    "enqueue_agent_run",
    "generate_report",
    "notify",
    "run_verification",
    "update_task",
];

/// Actions that a non-privileged author may not create.
pub const PRIVILEGED_ACTIONS: [&str; 1] = ["enqueue_agent_run"];

/// The trigger kinds a rule may have.
pub const TRIGGER_TYPES: [&str; 3] = ["event", "schedule", "manual"];

/// The catalog the automation tools return: triggers, schedule keys, actions and operators.
pub fn catalog() -> Value {
    let triggers: Map<String, Value> = TRIGGER_EVENTS
        .iter()
        .map(|(name, label)| ((*name).to_owned(), json!(label)))
        .collect();
    json!({
        "triggers": triggers,
        "schedule": ["cron", "interval_seconds"],
        "manual": true,
        "actions": ACTION_TYPES,
        "operators": OPERATORS.iter().map(|op| op.name()).collect::<Vec<_>>(),
    })
}

/// Validates a trigger document (`Trigger` in `rules.py`): one of the three kinds, event types
/// from [`TRIGGER_EVENTS`], exactly one of `cron` or `interval_seconds` for schedules (the cron
/// expression must parse), and an interval of at least 60 seconds.
pub fn validate_trigger(trigger: &Map<String, Value>) -> Result<(), WgError> {
    if let Some(extra) = trigger.keys().find(|key| {
        !matches!(
            key.as_str(),
            "type" | "event_types" | "cron" | "interval_seconds"
        )
    }) {
        return Err(WgError::validation(format!(
            "extra field {extra} is not permitted"
        )));
    }
    let interval = trigger
        .get("interval_seconds")
        .filter(|value| !value.is_null());
    if let Some(value) = interval {
        match value.as_i64() {
            Some(seconds) if seconds >= 60 => {}
            _ => {
                return Err(WgError::validation(
                    "interval_seconds must be an integer of at least 60",
                ));
            }
        }
    }
    let kind = trigger
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !TRIGGER_TYPES.contains(&kind) {
        return Err(WgError::validation(format!(
            "trigger type must be one of {TRIGGER_TYPES:?}"
        )));
    }
    match kind {
        "event" => {
            let types: Vec<&str> = trigger
                .get("event_types")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if types.is_empty() {
                return Err(WgError::validation(
                    "event triggers need at least one event type",
                ));
            }
            let allowed = |name: &str| TRIGGER_EVENTS.iter().any(|(event, _)| *event == name);
            let unknown: Vec<&str> = types
                .iter()
                .copied()
                .filter(|name| !allowed(name))
                .collect();
            if !unknown.is_empty() {
                return Err(WgError::validation(format!(
                    "unknown trigger events {unknown:?}"
                )));
            }
        }
        "schedule" => {
            let cron = trigger
                .get("cron")
                .filter(|value| truthy(value))
                .and_then(Value::as_str);
            let interval = interval.filter(|value| truthy(value));
            if cron.is_some() == interval.is_some() {
                return Err(WgError::validation(
                    "schedule triggers need exactly one of cron or interval_seconds",
                ));
            }
            if let Some(expression) = cron {
                Cron::parse(expression)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The task a context names, for action descriptions: its key, or the generic phrase.
fn task_text(context: &Value) -> String {
    match context
        .get("task")
        .filter(|task| truthy(task))
        .and_then(Value::as_object)
    {
        Some(task) => task
            .get("key")
            .map_or_else(|| "the triggering task".to_owned(), str_of),
        None => "the triggering task".to_owned(),
    }
}

/// One line describing what an action would do for the context (`AutomationService._describe`).
pub fn describe_action(action: &Map<String, Value>, context: &Value) -> String {
    let task = task_text(context);
    let kind = action
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let field = |name: &str| action.get(name).cloned().unwrap_or(Value::Null);
    match kind {
        "classify_task" => format!("request classification of {task}"),
        "add_tags" => format!("add tags {} to {task}", crate::py::repr(&field("tags"))),
        "update_task" => {
            let changes: Vec<String> = action
                .iter()
                .filter(|(key, _)| key.as_str() != "type")
                .map(|(key, value)| format!("{key}={}", str_of(value)))
                .collect();
            format!("update {task}: {}", changes.join(", "))
        }
        "create_child_tasks" => {
            let count = action
                .get("children")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("create {count} child task(s) under {task}")
        }
        "create_dependency" => format!("add dependency for {task}"),
        "enqueue_agent_run" => {
            let agent = action
                .get("agent")
                .filter(|value| truthy(value))
                .map_or_else(|| "default agent".to_owned(), str_of);
            format!("enqueue an agent run ({agent}) for {task}")
        }
        "run_verification" => format!("run verification checks for {task}"),
        "generate_report" => {
            let days = action
                .get("period_days")
                .filter(|value| truthy(value))
                .map_or_else(|| "7".to_owned(), str_of);
            format!("generate a {days}-day retrospective")
        }
        "notify" => {
            let title = action.get("title").map_or_else(String::new, str_of);
            format!("notify: {}", render_default(&title, context))
        }
        other => other.to_owned(),
    }
}

/// The dry-run record of each action: what it would do, without doing it.
pub fn preview_actions(actions: &[Map<String, Value>], context: &Value) -> Vec<Value> {
    actions
        .iter()
        .map(|action| {
            json!({
                "action": action,
                "planned": describe_action(action, context),
            })
        })
        .collect()
}
