//! The pure decisions of one automation run (`AutomationService.execute_run`): claiming the run,
//! the permissions and context its actions act with, the bodies of the task writes its actions
//! make, the notification an action records and the run's finish record.
//!
//! The shell ([`crate::service::execute`]) loads the run and its rows, runs each action through the
//! task services and applies what these decisions return. Nothing here reads a clock or the store.

use std::collections::BTreeSet;

use serde_json::{Map, Value as Json, json};
use wg_automation::template::render;
use wg_json::{PyStr, Value as Wire};
use wg_model::entity::{AutomationRule, AutomationRun, Notification};
use wg_model::event::{AutomationRunCompleted, NotificationCreated};
use wg_model::stamp::Stamp;
use wg_model::{Decision, EventDraft, EventPayload, Write};
use wg_runtimespec::RequestContext;
use wg_types::{Correlation, Id, Permission, WgError};

use super::{TriggerRows, condition_context, objects};

/// `AUTOMATION_PERMISSIONS`: what an automation run's actions may do. A rule a human authored also
/// gets `run:start` (`created_by` starts with `human:`).
pub fn run_permissions(created_by: &str) -> BTreeSet<Permission> {
    let mut permissions: BTreeSet<Permission> = [
        Permission::TaskRead,
        Permission::TaskWrite,
        Permission::WorkspaceRead,
        Permission::RepoRead,
        Permission::ClassifyRequest,
        Permission::VerifyRun,
        Permission::AnalyticsRead,
        Permission::GitRead,
    ]
    .into_iter()
    .collect();
    if created_by.starts_with("human:") {
        permissions.insert(Permission::RunStart);
    }
    permissions
}

/// A run and the rows its context reads, loaded by the shell.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub run: AutomationRun,
    pub rule: AutomationRule,
    pub rows: TriggerRows,
}

/// What a claimed run carries into its actions.
#[derive(Debug, Clone)]
pub struct Running {
    pub run_id: Id,
    pub workspace_id: Id,
    pub rule_name: String,
    /// The rule's actions, in order.
    pub actions: Vec<Map<String, Json>>,
    /// The indices of the actions a previous attempt finished.
    pub done: BTreeSet<usize>,
    /// The results a previous attempt kept (the succeeded ones).
    pub kept: Vec<Json>,
    /// The condition context the actions render their templates with.
    pub context: Json,
    pub depth: u32,
    pub permissions: BTreeSet<Permission>,
    pub correlation: Correlation,
    pub causation: Option<Id>,
}

/// The reply of [`claim`].
#[derive(Debug, Clone)]
pub enum Claim {
    /// The run is already settled (`succeeded`, `skipped`, `not_matched`); nothing runs.
    Finished(String),
    /// The run is now `running` and its attempt counted.
    Running(Box<Running>),
}

/// `execute_run`'s first transaction: a settled run is returned as it is; any other run is marked
/// `running` with one more attempt, and its actions are planned.
pub fn claim(stamp: &Stamp, loaded: &Loaded) -> Result<Decision<Claim>, WgError> {
    let status = loaded.run.status.as_str();
    if matches!(status, "succeeded" | "skipped" | "not_matched") {
        return Ok(Decision::new(Claim::Finished(status.to_owned())));
    }
    let context = condition_context(stamp.at, &loaded.run.inputs, loaded.rows.context());
    let kept: Vec<Json> = loaded
        .run
        .action_results
        .iter()
        .filter(|result| result.get("status").and_then(Json::as_str) == Some("succeeded"))
        .map(|result| Json::Object(result.clone()))
        .collect();
    let done = kept
        .iter()
        .filter_map(|result| result.get("index").and_then(Json::as_u64))
        .filter_map(|index| usize::try_from(index).ok())
        .collect();
    let rule_name = loaded.rule.name.clone();
    let running = Running {
        run_id: loaded.run.id,
        workspace_id: loaded.run.workspace_id,
        rule_name: rule_name.clone(),
        actions: loaded.rule.actions.iter().cloned().collect(),
        done,
        kept,
        context,
        depth: u32::try_from(loaded.run.depth).unwrap_or(u32::MAX),
        permissions: run_permissions(&loaded.rule.created_by),
        correlation: loaded
            .run
            .correlation_id
            .clone()
            .unwrap_or_else(|| stamp.correlation.clone()),
        causation: loaded.run.trigger_event_id,
    };
    let row = AutomationRun {
        status: "running".to_owned(),
        attempts: loaded.run.attempts + 1,
        ..loaded.run.clone()
    };
    Ok(Decision::new(Claim::Running(Box::new(running))).write(Write::update(row)))
}

/// The context an action runs in: the rule's automation actor, the run's correlation and the
/// trigger event as its cause.
pub fn action_context(running: &Running) -> Result<RequestContext, WgError> {
    RequestContext::automation(
        &running.rule_name,
        running.permissions.clone(),
        running.depth,
        running.correlation.clone(),
    )
    .map(|context| context.caused_by(running.causation))
}

/// The task an action works on: `context["task"]["id"]` (`_target_task`).
pub fn target_task(context: &Json) -> Result<String, WgError> {
    context
        .get("task")
        .and_then(|task| task.get("id"))
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or_else(|| WgError::validation("this action needs a task in its trigger context"))
}

/// The `update_task` body: the action's fields that are set, and the `automation` reason.
pub fn update_body(action: &Map<String, Json>) -> Map<String, Json> {
    let mut body: Map<String, Json> = action
        .iter()
        .filter(|(key, value)| key.as_str() != "type" && !value.is_null())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    body.insert("reason".to_owned(), json!("automation"));
    body
}

/// The `add_tags` body: the rendered tags.
pub fn tags_body(action: &Map<String, Json>, context: &Json) -> Map<String, Json> {
    let tags: Vec<String> = text_list(action, "tags")
        .map(|tag| render(tag, context, 100))
        .collect();
    Map::from_iter([("add_tags".to_owned(), json!(tags))])
}

/// The `create_child_tasks` children, in order.
pub fn children(action: &Map<String, Json>) -> Vec<Map<String, Json>> {
    action
        .get("children")
        .and_then(Json::as_array)
        .map(|items| items.iter().filter_map(Json::as_object).cloned().collect())
        .unwrap_or_default()
}

/// Whether a `create_child_tasks` action inherits the parent's repositories (default `true`).
pub fn inherits_repositories(action: &Map<String, Json>) -> bool {
    action
        .get("inherit_repositories")
        .and_then(Json::as_bool)
        .unwrap_or(true)
}

/// Whether a `create_child_tasks` action chains its children (default `false`).
pub fn is_sequential(action: &Map<String, Json>) -> bool {
    action
        .get("sequential")
        .and_then(Json::as_bool)
        .unwrap_or(false)
}

/// The `TaskCreate` body of one child: its rendered title and description, its priority
/// (`medium` when unset), its tags, the parent, the repositories and, when the children chain,
/// the previous child as its dependency.
pub fn child_body(
    child: &Map<String, Json>,
    parent: &str,
    repositories: &[String],
    previous: Option<&str>,
    context: &Json,
) -> Map<String, Json> {
    let title = render(text(child, "title"), context, 300);
    let description = render(text(child, "description"), context, 5000);
    let priority = child
        .get("priority")
        .and_then(Json::as_str)
        .unwrap_or("medium");
    let tags: Vec<String> = text_list(child, "tags").map(str::to_owned).collect();
    Map::from_iter([
        ("title".to_owned(), json!(title)),
        ("description".to_owned(), json!(description)),
        ("priority".to_owned(), json!(priority)),
        ("tags".to_owned(), json!(tags)),
        ("parent".to_owned(), json!(parent)),
        ("repositories".to_owned(), json!(repositories)),
        (
            "depends_on".to_owned(),
            json!(previous.map_or(Vec::new(), |id| vec![id])),
        ),
        ("status".to_owned(), json!("ready")),
    ])
}

/// The `create_dependency` request: the task that depends, and what it depends on. `depends_on`
/// makes the task depend on the rendered reference; `blocks` makes the reference depend on the task.
pub fn dependency_request(
    action: &Map<String, Json>,
    task: &str,
    context: &Json,
) -> (String, String) {
    match action.get("depends_on").and_then(Json::as_str) {
        Some(depends_on) => (task.to_owned(), render(depends_on, context, 100)),
        None => {
            let blocks = action.get("blocks").and_then(Json::as_str).unwrap_or("");
            (render(blocks, context, 100), task.to_owned())
        }
    }
}

/// The notification a `notify` action records, and its `notification.created` event.
pub fn notification(
    stamp: &Stamp,
    workspace_id: Id,
    action: &Map<String, Json>,
    context: &Json,
) -> Decision<Id> {
    let level = action
        .get("level")
        .and_then(Json::as_str)
        .unwrap_or("info")
        .to_owned();
    let title = render(text(action, "title"), context, 300);
    let body = render(text(action, "body"), context, 5000);
    let task_key = context
        .get("task")
        .and_then(|task| task.get("key"))
        .cloned()
        .unwrap_or(Json::Null);
    let note = Notification {
        id: stamp.id(0),
        workspace_id,
        level: level.clone(),
        title: title.clone(),
        body,
        source: stamp.actor.to_string(),
        data: Map::from_iter([("task".to_owned(), task_key)]),
        read_at: None,
        created_at: stamp.at,
    };
    let event = EventDraft::new(
        stamp,
        stamp.id(1),
        Some(workspace_id),
        note.id,
        EventPayload::NotificationCreated(NotificationCreated { level, title }),
    );
    Decision::new(note.id)
        .write(Write::insert(note.clone()))
        .event(event)
}

/// The end of a run: its action results, its status (`failed` when an action failed, else
/// `succeeded`) and the `automation.run_completed` event.
pub fn finish(
    stamp: &Stamp,
    run: &AutomationRun,
    rule_name: &str,
    results: &[Json],
    error: Option<String>,
) -> Decision<()> {
    let status = if error.is_some() {
        "failed"
    } else {
        "succeeded"
    };
    let event = EventDraft::new(
        stamp,
        stamp.id(0),
        Some(run.workspace_id),
        run.id,
        EventPayload::AutomationRunCompleted(AutomationRunCompleted {
            actions: json!(results.len()),
            error: error.clone(),
            rule: rule_name.to_owned(),
            status: status.to_owned(),
        }),
    );
    let row = AutomationRun {
        action_results: objects(results),
        status: status.to_owned(),
        error,
        finished_at: Some(stamp.at),
        ..run.clone()
    };
    Decision::new(()).write(Write::update(row)).event(event)
}

/// The Python value of a wire value, as the JSON the actions read it as (dates and models, which
/// no action reads back, become `null`).
pub fn to_json(value: &Wire) -> Json {
    match value {
        Wire::Null => Json::Null,
        Wire::Bool(flag) => Json::Bool(*flag),
        Wire::Int(number) => i64::try_from(*number).map_or(Json::Null, Json::from),
        Wire::Float(number) => {
            serde_json::Number::from_f64(*number).map_or(Json::Null, Json::Number)
        }
        Wire::Str(text) => Json::String(py_text(text)),
        Wire::List(items) => Json::Array(items.iter().map(to_json).collect()),
        Wire::Dict(items) => Json::Object(
            items
                .iter()
                .map(|(key, value)| (py_text(key), to_json(value)))
                .collect(),
        ),
        _ => Json::Null,
    }
}

fn py_text(text: &PyStr) -> String {
    text.code_points()
        .iter()
        .filter_map(|code| char::from_u32(*code))
        .collect()
}

/// A string member of an action, or the empty text when it is absent.
fn text<'a>(object: &'a Map<String, Json>, key: &str) -> &'a str {
    object.get(key).and_then(Json::as_str).unwrap_or("")
}

/// The string items of a list member of an action.
fn text_list<'a>(object: &'a Map<String, Json>, key: &str) -> impl Iterator<Item = &'a str> {
    object
        .get(key)
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
}
