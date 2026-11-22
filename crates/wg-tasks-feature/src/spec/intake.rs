//! The `task intake` replies (Pure): the proposed tasks per repository, the result of an action
//! on several keys, and their human views. The rules are `wg_tasks::intake`.

use std::collections::BTreeMap;

use serde_json::{Value as Json, json};
use wg_iface::cli::args::{get, items, python, s};
use wg_json::Value;
use wg_model::Task;
use wg_registry::resolve;
use wg_render::Console;
use wg_store::TaskWorld;
use wg_types::{Id, TaskStatus, WgError};
use wg_wire::convention::json_value;

/// The group of a proposal that names no repository.
const NO_REPOSITORY: &str = "(no repository)";

/// The proposed tasks of `workspace`, grouped by repository name (a task in several repositories
/// is listed under each). `repository` and `project` narrow the list; each task is a short row of
/// key, title, priority, tags and creator.
pub fn proposed_by_repository(
    world: &TaskWorld,
    workspace: Id,
    repository: Option<&str>,
    project: Option<&str>,
) -> Result<Json, WgError> {
    let tasks = &world.tasks;
    let repository = repository
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve::repository(&tasks.registry, workspace, reference, true))
        .transpose()?
        .map(|found| found.id);
    let project = project
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve::project(&tasks.registry, workspace, reference))
        .transpose()?
        .map(|found| found.id);
    let mut proposed: Vec<&Task> = tasks
        .tasks
        .values()
        .filter(|task| {
            task.workspace_id == workspace
                && task.status == TaskStatus::Proposed
                && task.archived_at.is_none()
        })
        .filter(|task| {
            repository.is_none_or(|id| {
                tasks
                    .repositories_of(task.id)
                    .any(|link| link.repository_id == id)
            })
        })
        .filter(|task| {
            project.is_none_or(|id| tasks.projects_of(task.id).any(|link| link.project_id == id))
        })
        .collect();
    proposed.sort_by_key(|task| (task.created_at, task.number));
    let mut groups: BTreeMap<String, Vec<Json>> = BTreeMap::new();
    for task in &proposed {
        let row = json!({
            "key": task.key.to_string(),
            "title": task.title,
            "priority": task.priority.as_str(),
            "tags": tasks.tags_of(task.id).collect::<Vec<_>>(),
            "created_by": task.created_by,
            "description": task.description,
            "acceptance_criteria": task.acceptance_criteria.iter().collect::<Vec<_>>(),
        });
        let names: Vec<String> = tasks
            .repositories_of(task.id)
            .filter_map(|link| tasks.registry.repository(link.repository_id))
            .map(|repo| repo.name.clone())
            .collect();
        if names.is_empty() {
            groups
                .entry(NO_REPOSITORY.to_owned())
                .or_default()
                .push(row);
        } else {
            for name in names {
                groups.entry(name).or_default().push(row.clone());
            }
        }
    }
    let repositories: Vec<Json> = groups
        .into_iter()
        .map(|(name, rows)| json!({"repository": name, "tasks": rows}))
        .collect();
    Ok(json!({"count": proposed.len(), "repositories": repositories}))
}

/// The reply of an intake action over several keys: one item per key, in the order given.
#[must_use]
pub fn action_report(action: &str, results: Vec<(String, Result<Json, WgError>)>) -> Value {
    let rows: Vec<Json> = results
        .into_iter()
        .map(|(key, outcome)| match outcome {
            Ok(detail) => json!({"key": key, "ok": true, "result": detail, "error": null}),
            Err(error) => json!({
                "key": key,
                "ok": false,
                "result": null,
                "error": error.message,
            }),
        })
        .collect();
    let failed = rows.iter().filter(|row| row["ok"] == json!(false)).count();
    json_value(&json!({
        "action": action,
        "done": rows.len() - failed,
        "failed": failed,
        "results": rows,
    }))
}

/// Whether any key of an action report failed, which makes the command exit 3.
#[must_use]
pub fn action_failed(report: &Value) -> bool {
    matches!(get(report, "failed"), Some(Value::Int(n)) if *n > 0)
}

/// The human view of `task intake list`: a table per repository.
#[must_use]
pub fn list_view(console: &Console, listing: &Value) -> String {
    let groups = items(get(listing, "repositories"));
    if groups.is_empty() {
        return console.print("no proposed tasks").text;
    }
    let mut out = String::new();
    for group in groups {
        let rows: Vec<Vec<String>> = items(get(group, "tasks"))
            .iter()
            .map(|task| {
                vec![
                    s(get(task, "key")),
                    s(get(task, "priority")),
                    s(get(task, "title")),
                    s(get(task, "created_by")),
                ]
            })
            .collect();
        let title = format!("proposed: {}", s(get(group, "repository")));
        out.push_str(
            &console
                .table(
                    &rows,
                    &["key", "priority", "title", "created by"],
                    Some(&title),
                )
                .text,
        );
    }
    let count = get(listing, "count").map_or_else(String::new, python);
    out.push_str(&console.print(&format!("{count} proposed")).text);
    out
}

/// The human view of an intake action: one row per key.
#[must_use]
pub fn action_view(console: &Console, report: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(report, "results"))
        .iter()
        .map(|item| {
            let ok = matches!(get(item, "ok"), Some(Value::Bool(true)));
            vec![
                s(get(item, "key")),
                if ok { "ok" } else { "error" }.to_owned(),
                if ok {
                    "-".to_owned()
                } else {
                    s(get(item, "error"))
                },
            ]
        })
        .collect();
    let title = format!("intake {}", s(get(report, "action")));
    let mut out = console
        .table(&rows, &["key", "result", "error"], Some(&title))
        .text;
    out.push_str(
        &console
            .print(&format!(
                "{} done, {} failed",
                get(report, "done").map_or_else(String::new, python),
                get(report, "failed").map_or_else(String::new, python)
            ))
            .text,
    );
    out
}
