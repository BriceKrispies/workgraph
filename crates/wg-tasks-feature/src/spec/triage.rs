//! The `task triage-apply` report (Pure): the JSON of a triage run (`{applied, skipped, errors}`)
//! and its human table. The plan and the steps are `wg_tasks::triage`; this module only shapes
//! what they came to.

use serde_json::{Value as Json, json};
use wg_clispec::args::{get, items, python, s};
use wg_json::Value;
use wg_model::Task;
use wg_render::Console;
use wg_store::TaskWorld;
use wg_tasks::triage::{Failure, FollowUp, PlannedVerdict, Skipped, TriageTarget};
use wg_types::Id;

const HEAD: [&str; 4] = ["key", "verdict", "action", "new keys / error"];

/// The fields a triage plan reads of a task: its key, status, priority, tags and memberships.
#[must_use]
pub fn triage_target(world: &TaskWorld, task: &Task) -> TriageTarget {
    TriageTarget {
        key: task.key.to_string(),
        status: task.status,
        priority: task.priority,
        tags: world.tasks.tags_of(task.id).map(str::to_owned).collect(),
        projects: world
            .tasks
            .projects_of(task.id)
            .map(|link| link.project_id)
            .collect(),
        repositories: world
            .tasks
            .repositories_of(task.id)
            .map(|link| link.repository_id)
            .collect(),
    }
}

/// The `TaskCreate` body of a follow-up: the rewrite's text, its files and the original's
/// priority, projects and repositories.
#[must_use]
pub fn follow_up_body(follow: &FollowUp) -> Json {
    let files: Vec<Json> = follow
        .files
        .iter()
        .map(|file| match &file.anchor {
            Some(anchor) => json!({"path": file.path, "anchor": anchor}),
            None => json!({"path": file.path}),
        })
        .collect();
    json!({
        "title": follow.title,
        "description": follow.description,
        "acceptance_criteria": follow.acceptance_criteria,
        "priority": follow.priority.as_str(),
        "tags": follow.tags,
        "files": files,
        "projects": follow.projects.iter().map(Id::to_string).collect::<Vec<_>>(),
        "repositories": follow.repositories.iter().map(Id::to_string).collect::<Vec<_>>(),
    })
}

/// The key of a task a service just created, read from its reply.
#[must_use]
pub fn created_key(reply: &Value) -> String {
    s(get(reply, "key"))
}

/// One applied verdict. `new_keys` are the follow-up tasks it created, in order.
#[must_use]
pub fn applied_item(planned: &PlannedVerdict, new_keys: &[String]) -> Json {
    json!({
        "key": planned.key,
        "verdict": planned.verdict.as_str(),
        "action": planned.action.as_str(),
        "new_keys": new_keys,
        "error": null,
    })
}

/// A verdict skipped because its task was already triaged.
#[must_use]
pub fn skipped_item(skipped: &Skipped) -> Json {
    json!({
        "key": skipped.key,
        "verdict": skipped.verdict.as_str(),
        "reason": skipped.reason,
    })
}

/// A rejected element of the file, as planning recorded it (`key` and `verdict` as written).
#[must_use]
pub fn failure_item(failure: &Failure) -> Json {
    json!({
        "index": failure.index,
        "key": failure.key,
        "verdict": failure.verdict,
        "error": failure.error,
    })
}

/// A planned verdict whose steps failed part way through.
#[must_use]
pub fn step_failure_item(planned: &PlannedVerdict, error: &str) -> Json {
    json!({
        "index": planned.index,
        "key": planned.key,
        "verdict": planned.verdict.as_str(),
        "error": error,
    })
}

/// The report: `applied`, `skipped` and `errors`, in that key order.
#[must_use]
pub fn report(applied: Vec<Json>, skipped: Vec<Json>, errors: Vec<Json>) -> Json {
    json!({
        "applied": applied,
        "skipped": skipped,
        "errors": errors,
    })
}

/// The human view of `task triage-apply`: one row per verdict, then the counts. A dry run says
/// that nothing changed.
#[must_use]
pub fn triage_view(console: &Console, report: &Value, dry_run: bool) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for item in items(get(report, "applied")) {
        let new_keys: Vec<String> = items(get(item, "new_keys")).iter().map(python).collect();
        rows.push(vec![
            cell(get(item, "key")),
            cell(get(item, "verdict")),
            cell(get(item, "action")),
            if new_keys.is_empty() {
                "-".to_owned()
            } else {
                new_keys.join(", ")
            },
        ]);
    }
    for item in items(get(report, "skipped")) {
        rows.push(vec![
            cell(get(item, "key")),
            cell(get(item, "verdict")),
            "skipped".to_owned(),
            cell(get(item, "reason")),
        ]);
    }
    for item in items(get(report, "errors")) {
        rows.push(vec![
            cell(get(item, "key")),
            cell(get(item, "verdict")),
            "error".to_owned(),
            cell(get(item, "error")),
        ]);
    }
    let title = if dry_run {
        "triage (dry run)"
    } else {
        "triage"
    };
    let mut out = console.table(&rows, &HEAD, Some(title)).text;
    let counts = format!(
        "{} applied, {} skipped, {} errors{}",
        items(get(report, "applied")).len(),
        items(get(report, "skipped")).len(),
        items(get(report, "errors")).len(),
        if dry_run { " (nothing changed)" } else { "" }
    );
    out.push_str(&console.print(&counts).text);
    out
}

/// Whether the run had any error, which makes `task triage-apply` exit 1.
#[must_use]
pub fn triage_failed(report: &Value) -> bool {
    !items(get(report, "errors")).is_empty()
}

/// A table cell: the value as Python prints it, or `-` when there is none (absent, null, empty).
fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".to_owned(),
        Some(Value::Str(text)) if text.code_points().is_empty() => "-".to_owned(),
        Some(other) => python(other),
    }
}
