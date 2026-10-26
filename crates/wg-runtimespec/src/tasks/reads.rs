//! The task reads: list, ready queue, detail, graph and history (`TaskService.list`,
//! `get_ready`, `detail`, `graph` and `HistoryService.task_history`).

use std::collections::BTreeSet;

use wg_json::Value;
use wg_model::entity::{Task, TaskTag};
use wg_registry::resolve;
use wg_storecodec::EventRecord;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::graph::{GraphEdgeKind, GraphQuery, task_graph};
use wg_tasks::promote::{ReadyQuery, ready_tasks};
use wg_types::{Id, TaskPriority, WgError};
use wg_wire::convention::{TimeFormat, id, json_object, model, optional_id, text, timestamp};
use wg_wire::dto::graph_edge::{self, EdgeKind};
use wg_wire::dto::{graph_node, page, task_graph as task_graph_dto};

use super::out::{task_detail_out, task_out};
use super::requests::FilterRead;

/// `to_fts_query(text)` with its defaults: each token quoted, the last one a prefix, AND-ed.
/// `None` when the text holds no word characters.
#[must_use]
pub fn fts_match(text: &str) -> Option<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    let last = tokens.len().checked_sub(1)?;
    let quoted: Vec<String> = tokens
        .iter()
        .enumerate()
        .map(|(index, token)| {
            if index == last {
                format!("\"{token}\"*")
            } else {
                format!("\"{token}\"")
            }
        })
        .collect();
    Some(quoted.join(" "))
}

/// The `MATCH` expression a list filter's text search needs run, if it has one.
#[must_use]
pub fn filter_match(filter: &FilterRead) -> Option<String> {
    filter.text.as_deref().and_then(fts_match)
}

fn priority_rank(priority: TaskPriority) -> i64 {
    match priority {
        TaskPriority::Critical => 0,
        TaskPriority::High => 1,
        TaskPriority::Medium => 2,
        TaskPriority::Low => 3,
    }
}

/// The tasks of `workspace` that pass `filter`, unsorted.
fn filtered<'a>(
    world: &'a TaskWorld,
    workspace: Id,
    filter: &FilterRead,
    fts: Option<&BTreeSet<Id>>,
) -> Result<Vec<&'a Task>, WgError> {
    let tasks = &world.tasks;
    let project = filter
        .project
        .as_deref()
        .map(|reference| resolve::project(&tasks.registry, workspace, reference))
        .transpose()?
        .map(|project| project.id);
    let repository = filter
        .repository
        .as_deref()
        .map(|reference| resolve::repository(&tasks.registry, workspace, reference, true))
        .transpose()?
        .map(|repository| repository.id);
    let parent = filter
        .parent
        .as_deref()
        .map(|reference| resolve::task(tasks, Some(workspace), reference))
        .transpose()?
        .map(|parent| parent.id);
    Ok(tasks
        .tasks
        .values()
        .filter(|task| {
            task.workspace_id == workspace
                && if filter.archived_only {
                    task.archived_at.is_some()
                } else {
                    filter.include_archived || task.archived_at.is_none()
                }
                && (filter.statuses.is_empty() || filter.statuses.contains(&task.status))
                && (filter.priorities.is_empty() || filter.priorities.contains(&task.priority))
                && filter
                    .category
                    .as_ref()
                    .is_none_or(|category| task.category.as_ref() == Some(category))
                && filter
                    .assignee
                    .as_ref()
                    .is_none_or(|assignee| task.assignee.as_ref() == Some(assignee))
                && project
                    .is_none_or(|project| tasks.task_projects.contains_key(&(task.id, project)))
                && filter
                    .file
                    .as_ref()
                    .is_none_or(|path| tasks.files_of(task.id).any(|file| &file.path == path))
                && repository.is_none_or(|repository| {
                    tasks.task_repositories.contains_key(&(task.id, repository))
                })
                && filter
                    .tags
                    .iter()
                    .all(|tag| tasks.tags_of(task.id).any(|have| have == tag))
                && parent.is_none_or(|parent| task.parent_id == Some(parent))
                && (!filter.root_only || task.parent_id.is_none())
                && filter
                    .created_after
                    .is_none_or(|after| task.created_at >= after)
                && filter
                    .created_before
                    .is_none_or(|before| task.created_at < before)
                && fts.is_none_or(|ids| ids.contains(&task.id))
        })
        .collect())
}

/// `TaskService.list`: a `Page[TaskOut]`. `fts` holds the ids the text search matched, when the
/// filter has a text search with words in it.
pub fn list_tasks(
    world: &TaskWorld,
    workspace: Id,
    filter: &FilterRead,
    fts: Option<&BTreeSet<Id>>,
) -> Result<Value, WgError> {
    let mut found = filtered(world, workspace, filter, fts)?;
    let total = i64::try_from(found.len()).unwrap_or(i64::MAX);
    match filter.sort.as_str() {
        "priority" => found.sort_by_key(|task| {
            let rank = priority_rank(task.priority);
            (if filter.descending { -rank } else { rank }, task.number)
        }),
        "key" => found.sort_by_key(|task| {
            if filter.descending {
                -task.number
            } else {
                task.number
            }
        }),
        other => {
            found.sort_by(|a, b| {
                let ordering = match other {
                    "updated_at" => a.updated_at.cmp(&b.updated_at),
                    "title" => a.title.cmp(&b.title),
                    "status" => a.status.as_str().cmp(b.status.as_str()),
                    "due_at" => a.due_at.cmp(&b.due_at),
                    _ => a.created_at.cmp(&b.created_at),
                };
                let ordering = if filter.descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                ordering.then(b.number.cmp(&a.number))
            });
        }
    }
    let skip = usize::try_from(filter.offset).unwrap_or(0);
    let take = usize::try_from(filter.limit).unwrap_or(0);
    let items: Vec<Value> = found
        .into_iter()
        .skip(skip)
        .take(take)
        .map(|task| task_out(&world.tasks, task, None))
        .collect();
    let next = filter.offset + filter.limit;
    Ok(page::from_model(
        items,
        total,
        filter.limit,
        filter.offset,
        (next < total).then_some(next),
    ))
}

/// `TaskService.get_ready`: ready tasks whose dependencies are accepted, best first.
pub fn ready(
    world: &TaskWorld,
    workspace: Id,
    project: Option<&str>,
    repository: Option<&str>,
    agent: Option<&str>,
    limit: i64,
) -> Result<Value, WgError> {
    let tasks = &world.tasks;
    let project = project
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve::project(&tasks.registry, workspace, reference))
        .transpose()?
        .map(|project| project.id);
    let repository = repository
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve::repository(&tasks.registry, workspace, reference, true))
        .transpose()?
        .map(|repository| repository.id);
    let found = ready_tasks(
        tasks,
        &world.stacks,
        ReadyQuery {
            workspace_id: workspace,
            project,
            repository,
            agent: agent.filter(|name| !name.is_empty()),
            limit: usize::try_from(limit).unwrap_or(0).max(1),
        },
    );
    Ok(Value::List(
        found
            .into_iter()
            .map(|task| task_out(tasks, task, None))
            .collect(),
    ))
}

/// What `task graph` is asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct GraphRequest<'a> {
    pub root: Option<&'a str>,
    pub project: Option<&'a str>,
    pub repository: Option<&'a str>,
    pub include_closed: bool,
}

/// `TaskService.graph`: a `TaskGraph`.
pub fn graph(
    world: &TaskWorld,
    workspace: Id,
    request: &GraphRequest<'_>,
) -> Result<Value, WgError> {
    let tasks = &world.tasks;
    let root = request
        .root
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve::task(tasks, Some(workspace), reference))
        .transpose()?
        .map(|task| task.id);
    let project = request
        .project
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve::project(&tasks.registry, workspace, reference))
        .transpose()?
        .map(|project| project.id);
    let repository = request
        .repository
        .filter(|reference| !reference.is_empty())
        .map(|reference| resolve::repository(&tasks.registry, workspace, reference, true))
        .transpose()?
        .map(|repository| repository.id);
    let built = task_graph(
        tasks,
        &world.stacks,
        &GraphQuery {
            workspace_id: workspace,
            project,
            repository,
            root,
            include_closed: request.include_closed,
        },
    );
    let nodes: Vec<Value> = built
        .nodes
        .iter()
        .filter_map(|node| {
            tasks.task(node.id).map(|task| {
                let shown = Task {
                    parent_id: node.parent_id,
                    ..task.clone()
                };
                let tags: Vec<TaskTag> = node
                    .tags
                    .iter()
                    .map(|tag| TaskTag {
                        task_id: node.id,
                        tag: tag.clone(),
                    })
                    .collect();
                graph_node::from_model(
                    &shown,
                    &node.project_ids,
                    &node.repository_ids,
                    &tags,
                    node.blocked,
                )
            })
        })
        .collect();
    let edges: Vec<Value> = built
        .edges
        .iter()
        .map(|edge| {
            graph_edge::from_model(
                edge.source,
                edge.target,
                match edge.kind {
                    GraphEdgeKind::DependsOn => EdgeKind::DependsOn,
                    GraphEdgeKind::Parent => EdgeKind::Parent,
                    GraphEdgeKind::Relation => EdgeKind::Relation,
                },
                edge.relation_type.map(|kind| kind.as_str()),
            )
        })
        .collect();
    Ok(task_graph_dto::from_model(nodes, edges))
}

/// `resolve.task`: the id of the task `reference` names.
pub fn resolve_task_id(
    world: &TaskWorld,
    workspace: Option<Id>,
    reference: &str,
) -> Result<Id, WgError> {
    resolve::task(&world.tasks, workspace, reference).map(|task| task.id)
}

/// The tasks `show` can resolve, so the shell can load their comments.
#[must_use]
pub fn shown_ids(world: &TaskWorld, workspace: Option<Id>, refs: &[String]) -> Vec<Id> {
    refs.iter()
        .filter_map(|reference| resolve::task(&world.tasks, workspace, reference).ok())
        .map(|task| task.id)
        .collect()
}

/// What `task show` gathered: the details found and the references that failed.
#[derive(Debug, Clone)]
pub struct Shown {
    pub items: Vec<Value>,
    pub errors: Vec<(String, WgError)>,
}

/// `TaskService.detail` for each reference; one that fails does not stop the others.
#[must_use]
pub fn show(world: &TaskWorld, workspace: Option<Id>, refs: &[String]) -> Shown {
    let mut shown = Shown {
        items: Vec::new(),
        errors: Vec::new(),
    };
    for reference in refs {
        match resolve::task(&world.tasks, workspace, reference) {
            Ok(task) => shown
                .items
                .push(task_detail_out(&world.tasks, &world.stacks, task)),
            Err(error) => shown.errors.push((reference.clone(), error)),
        }
    }
    shown
}

/// `EventOut` for a stored event.
#[must_use]
pub fn event_out(event: &EventRecord) -> Value {
    model(
        "EventOut",
        vec![
            ("seq", Value::Int(i128::from(event.seq))),
            ("id", id(event.id)),
            ("workspace_id", optional_id(event.workspace_id)),
            ("event_type", text(&event.event_type)),
            ("entity_type", text(&event.entity_type)),
            ("entity_id", id(event.entity_id)),
            ("actor", text(&event.actor)),
            ("actor_kind", text(&event.actor_kind)),
            (
                "occurred_at",
                timestamp(event.occurred_at, TimeFormat::ModelZ),
            ),
            ("correlation_id", id(event.correlation_id)),
            ("causation_id", optional_id(event.causation_id)),
            ("attempt_id", optional_id(event.attempt_id)),
            (
                "schema_version",
                Value::Int(i128::from(event.schema_version)),
            ),
            ("data", json_object(&event.data)),
        ],
    )
}
