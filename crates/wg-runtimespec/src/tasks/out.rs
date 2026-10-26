//! Task output values: `TaskOut`, `TaskDetail`, the rollup and the dependency state, assembled
//! from a task slice the way `task_out` and `TaskService.detail` do.

use std::collections::BTreeSet;

use wg_json::Value;
use wg_model::TaskSlice;
use wg_model::entity::{Task, TaskComment, TaskFile, TaskLink, TaskRelation, TaskTag};
use wg_model::slice::StackSlice;
use wg_tasks::deps::unsatisfied_dependencies;
use wg_tasks::hierarchy::rollup;
use wg_types::Id;
use wg_wire::dto::task::{self, TaskRelated};
use wg_wire::dto::{dependency_state, task_brief, task_detail, task_rollup};

/// The ids of a task's satellite rows, which `TaskOut` lists.
struct Ids {
    projects: Vec<Id>,
    repositories: Vec<Id>,
    children: Vec<Id>,
    depends_on: Vec<Id>,
    blocks: Vec<Id>,
}

fn sorted(ids: impl Iterator<Item = Id>) -> Vec<Id> {
    ids.collect::<BTreeSet<_>>().into_iter().collect()
}

fn ids_of(tasks: &TaskSlice, task: &Task) -> Ids {
    let mut children: Vec<&Task> = tasks
        .tasks
        .values()
        .filter(|other| other.parent_id == Some(task.id))
        .collect();
    children.sort_by_key(|child| child.number);
    Ids {
        projects: sorted(tasks.projects_of(task.id).map(|row| row.project_id)),
        repositories: sorted(tasks.repositories_of(task.id).map(|row| row.repository_id)),
        children: children.iter().map(|child| child.id).collect(),
        depends_on: sorted(tasks.dependencies_of(task.id).map(|row| row.depends_on_id)),
        blocks: sorted(tasks.dependents_of(task.id).map(|row| row.task_id)),
    }
}

/// What a `TaskOut` lists, read from the slice.
struct Rows {
    tags: Vec<TaskTag>,
    files: Vec<TaskFile>,
    links: Vec<TaskLink>,
    ids: Ids,
}

fn rows_of(tasks: &TaskSlice, task: &Task, tag_order: Option<&[String]>) -> Rows {
    let tags: Vec<TaskTag> = match tag_order {
        Some(order) => order.to_vec(),
        None => tasks.tags_of(task.id).map(str::to_owned).collect(),
    }
    .into_iter()
    .map(|tag| TaskTag {
        task_id: task.id,
        tag,
    })
    .collect();
    let mut links: Vec<TaskLink> = tasks.links_of(task.id).cloned().collect();
    links.sort_by(|a, b| (&a.kind, &a.url).cmp(&(&b.kind, &b.url)));
    Rows {
        tags,
        files: tasks.files_of(task.id).cloned().collect(),
        links,
        ids: ids_of(tasks, task),
    }
}

/// `task_out`: the task with its tags, files, links and related ids. `tag_order` is the tag list
/// as a write just assigned it; Python returns that order from the write itself and the sorted
/// order from every later read.
#[must_use]
pub fn task_out(tasks: &TaskSlice, task: &Task, tag_order: Option<&[String]>) -> Value {
    let rows = rows_of(tasks, task, tag_order);
    task::from_model(
        task,
        &TaskRelated {
            tags: &rows.tags,
            files: &rows.files,
            links: &rows.links,
            project_ids: &rows.ids.projects,
            repository_ids: &rows.ids.repositories,
            child_ids: &rows.ids.children,
            depends_on: &rows.ids.depends_on,
            blocks: &rows.ids.blocks,
        },
    )
}

/// The `key` field of a task output.
#[must_use]
pub fn key_of(out: &Value) -> Option<String> {
    match out {
        Value::Model { fields, .. } => fields.iter().find_map(|(name, value)| match value {
            Value::Str(text) if name == "key" => Some(
                text.code_points()
                    .iter()
                    .filter_map(|point| char::from_u32(*point))
                    .collect(),
            ),
            _ => None,
        }),
        _ => None,
    }
}

/// `BulkResult`: the keys updated and the message of each reference that failed.
#[must_use]
pub fn bulk_result_out(result: &wg_tasks::bulk::BulkResult) -> Value {
    wg_wire::convention::model(
        "BulkResult",
        vec![
            (
                "updated",
                Value::List(
                    result
                        .updated
                        .iter()
                        .map(|key| wg_wire::convention::text(key))
                        .collect(),
                ),
            ),
            (
                "failed",
                Value::Dict(
                    result
                        .failed
                        .iter()
                        .map(|(reference, message)| {
                            (
                                wg_json::PyStr::from(reference.as_str()),
                                wg_wire::convention::text(message),
                            )
                        })
                        .collect(),
                ),
            ),
        ],
    )
}

/// `TaskService._rollup`.
#[must_use]
pub fn rollup_out(tasks: &TaskSlice, task: &Task) -> Option<Value> {
    rollup(tasks, task.id).map(|found| {
        let by_status: Vec<(String, i64)> = found
            .by_status
            .iter()
            .map(|(status, count)| {
                (
                    status.as_str().to_owned(),
                    i64::try_from(*count).unwrap_or(i64::MAX),
                )
            })
            .collect();
        task_rollup::from_model(
            i64::try_from(found.total_descendants).unwrap_or(i64::MAX),
            &by_status,
            i64::try_from(found.accepted).unwrap_or(i64::MAX),
            found.progress,
        )
    })
}

/// `TaskService._dependency_state`.
#[must_use]
pub fn dependency_state_out(tasks: &TaskSlice, stacks: &StackSlice, task: &Task) -> Value {
    let blockers = unsatisfied_dependencies(tasks, stacks, task.id);
    dependency_state::from_model(
        blockers.is_empty(),
        blockers.into_iter().map(task_brief::from_model).collect(),
    )
}

/// `TaskService.detail`: `TaskOut` plus relations, comments, rollup and dependency state. The
/// slice must hold the task's comments.
#[must_use]
pub fn task_detail_out(tasks: &TaskSlice, stacks: &StackSlice, task: &Task) -> Value {
    let rows = rows_of(tasks, task, None);
    let mut relations: Vec<TaskRelation> = tasks
        .relations
        .values()
        .filter(|row| row.source_task_id == task.id || row.target_task_id == task.id)
        .cloned()
        .collect();
    relations.sort_by_key(|row| (row.created_at, row.id));
    let mut comments: Vec<TaskComment> = tasks.comments_of(task.id).cloned().collect();
    comments.sort_by_key(|row| (row.created_at, row.id));
    task_detail::from_model(
        task,
        &TaskRelated {
            tags: &rows.tags,
            files: &rows.files,
            links: &rows.links,
            project_ids: &rows.ids.projects,
            repository_ids: &rows.ids.repositories,
            child_ids: &rows.ids.children,
            depends_on: &rows.ids.depends_on,
            blocks: &rows.ids.blocks,
        },
        &relations,
        &comments,
        rollup_out(tasks, task),
        Some(dependency_state_out(tasks, stacks, task)),
    )
}
