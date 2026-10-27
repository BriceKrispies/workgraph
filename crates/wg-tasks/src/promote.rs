//! Dependency propagation, promotion and the ready queue (GV-19). Mirrors the dependency gate of
//! `_transition`, `_refresh_blocked_state`, `promote_ready`, `promote_dependents`,
//! `_backlog_promotable`, `_promoted_by_project` and `get_ready` in `core/services/tasks.py`.
//!
//! Each function answers what should happen; the shell applies the status changes it names.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use wg_model::slice::StackSlice;
use wg_model::{Task, TaskSlice};
use wg_types::{Id, TaskPriority, TaskStatus, WgError};

use crate::deps::{BLOCKED_BY_DEPENDENCIES, satisfied_dependencies, unsatisfied_dependencies};
use crate::lifecycle::requires_dependencies;

/// Tasks tagged this are never promoted automatically.
pub const PARKED_TAG: &str = "parked";

/// Reason recorded when a blocked task becomes ready.
pub const REASON_UNBLOCKED: &str = "all dependencies accepted";
/// Reason recorded when a ready task is blocked again by a reopened dependency.
pub const REASON_REOPENED: &str = "dependency reopened";
/// Reason recorded when promotion moves a backlog task to ready.
pub const REASON_PROMOTED: &str = "dependencies accepted";

/// A requested status change after the dependency gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gated {
    /// The status to move to: the requested one, or `blocked` when `ready` was refused.
    pub target: TaskStatus,
    /// The reason to record; when blocked, defaults to the list of blockers.
    pub reason: Option<String>,
    /// Whether the task must get `blocked_reason = "dependencies"` in its metadata.
    pub blocked_by_dependencies: bool,
}

/// The dependency gate of a status change. Moving to `ready`, `claimed` or `running` needs every
/// dependency satisfied; `ready` with blockers turns into `blocked`, the others are refused.
pub fn dependency_gate(
    tasks: &TaskSlice,
    stacks: &StackSlice,
    task: &Task,
    target: TaskStatus,
    reason: Option<&str>,
) -> Result<Gated, WgError> {
    let reason = reason.filter(|text| !text.is_empty()).map(str::to_owned);
    if !requires_dependencies(target) {
        return Ok(Gated {
            target,
            reason,
            blocked_by_dependencies: false,
        });
    }
    let blockers = unsatisfied_dependencies(tasks, stacks, task.id);
    if blockers.is_empty() {
        return Ok(Gated {
            target,
            reason,
            blocked_by_dependencies: false,
        });
    }
    let keys: Vec<String> = blockers.iter().map(|b| b.key.to_string()).collect();
    if target != TaskStatus::Ready {
        return Err(WgError::invalid_transition(format!(
            "task {} has unsatisfied dependencies",
            task.key
        ))
        .detail("blocked_by", keys));
    }
    Ok(Gated {
        target: TaskStatus::Blocked,
        reason: reason.or_else(|| Some(format!("dependencies unsatisfied: {}", keys.join(", ")))),
        blocked_by_dependencies: true,
    })
}

/// A status change the shell must run through [`dependency_gate`] and apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refresh {
    pub target: TaskStatus,
    pub reason: &'static str,
}

/// What a change to a dependency of `task` does to it: a blocked task parked by dependencies
/// whose blockers are gone becomes ready; a ready task with blockers is blocked again.
pub fn refresh_blocked_state(
    tasks: &TaskSlice,
    stacks: &StackSlice,
    task: &Task,
) -> Option<Refresh> {
    let blocked = !unsatisfied_dependencies(tasks, stacks, task.id).is_empty();
    match task.status {
        TaskStatus::Blocked
            if !blocked
                && task.metadata.get("blocked_reason").and_then(Value::as_str)
                    == Some(BLOCKED_BY_DEPENDENCIES) =>
        {
            Some(Refresh {
                target: TaskStatus::Ready,
                reason: REASON_UNBLOCKED,
            })
        }
        TaskStatus::Ready if blocked => Some(Refresh {
            target: TaskStatus::Ready,
            reason: REASON_REOPENED,
        }),
        _ => None,
    }
}

/// The tasks that depend on `task_id`, by task number: the ones to refresh when it is accepted
/// or reopened.
pub fn dependents(tasks: &TaskSlice, task_id: Id) -> Vec<&Task> {
    tasks
        .dependents_of(task_id)
        .filter_map(|row| tasks.task(row.task_id))
        .map(|task| (task.number, task))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// The backlog tasks among `candidates` that may move to ready: no children, not parked, and
/// every dependency satisfied. Order is kept.
pub fn backlog_promotable<'a>(
    tasks: &TaskSlice,
    stacks: &StackSlice,
    candidates: &[&'a Task],
) -> Vec<&'a Task> {
    let backlog: BTreeSet<Id> = candidates
        .iter()
        .filter(|task| task.status == TaskStatus::Backlog)
        .map(|task| task.id)
        .collect();
    let with_children: BTreeSet<Id> = tasks
        .tasks
        .values()
        .filter_map(|task| task.parent_id)
        .filter(|parent| backlog.contains(parent))
        .collect();
    let parked: BTreeSet<Id> = tasks
        .tags
        .values()
        .filter(|row| row.tag == PARKED_TAG && backlog.contains(&row.task_id))
        .map(|row| row.task_id)
        .collect();
    let edges: Vec<(Id, Id)> = tasks
        .dependencies
        .values()
        .filter(|row| backlog.contains(&row.task_id))
        .map(|row| (row.task_id, row.depends_on_id))
        .collect();
    let satisfied = satisfied_dependencies(tasks, stacks, &edges);
    let blocked: BTreeSet<Id> = edges
        .iter()
        .filter(|edge| !satisfied.contains(edge))
        .map(|(task_id, _)| *task_id)
        .collect();
    candidates
        .iter()
        .copied()
        .filter(|task| {
            task.status == TaskStatus::Backlog
                && !with_children.contains(&task.id)
                && !parked.contains(&task.id)
                && !blocked.contains(&task.id)
        })
        .collect()
}

/// Narrowing of the promotion candidates.
#[derive(Debug, Clone, Copy, Default)]
pub struct PromoteScope<'a> {
    /// A candidate carries every one of these tags.
    pub tags: &'a [String],
    /// A candidate belongs to this project.
    pub project: Option<Id>,
}

/// The backlog tasks of a workspace that `promote_ready` moves to ready, by task number.
/// Archived tasks are never candidates.
pub fn promote_ready<'a>(
    tasks: &'a TaskSlice,
    stacks: &StackSlice,
    workspace_id: Id,
    scope: PromoteScope<'_>,
) -> Vec<&'a Task> {
    let candidates: Vec<&Task> = tasks
        .tasks
        .values()
        .filter(|task| {
            task.workspace_id == workspace_id
                && task.archived_at.is_none()
                && task.status == TaskStatus::Backlog
                && scope
                    .tags
                    .iter()
                    .all(|tag| tasks.tags_of(task.id).any(|have| have == tag))
                && scope
                    .project
                    .is_none_or(|project| tasks.task_projects.contains_key(&(task.id, project)))
        })
        .map(|task| (task.number, task))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    backlog_promotable(tasks, stacks, &candidates)
}

/// The backlog dependents of `task_id` that become promotable once it is accepted, by task
/// number.
pub fn promote_dependents<'a>(
    tasks: &'a TaskSlice,
    stacks: &StackSlice,
    task_id: Id,
) -> Vec<&'a Task> {
    let candidates: Vec<&Task> = dependents(tasks, task_id)
        .into_iter()
        .filter(|task| task.status == TaskStatus::Backlog && task.archived_at.is_none())
        .collect();
    backlog_promotable(tasks, stacks, &candidates)
}

/// Promoted tasks counted per project slug; a task in several projects counts once under each,
/// a task in none under `"(none)"`.
pub fn promoted_by_project(tasks: &TaskSlice, promoted: &[&Task]) -> BTreeMap<String, usize> {
    promoted
        .iter()
        .flat_map(|task| {
            let slugs: Vec<String> = tasks
                .projects_of(task.id)
                .filter_map(|row| tasks.registry.project(row.project_id))
                .map(|project| project.slug.clone())
                .collect();
            if slugs.is_empty() {
                vec!["(none)".to_owned()]
            } else {
                slugs
            }
        })
        .fold(BTreeMap::new(), |acc, slug| {
            let count = acc.get(&slug).copied().unwrap_or(0) + 1;
            acc.into_iter()
                .chain(std::iter::once((slug, count)))
                .collect()
        })
}

/// Narrowing of the ready queue.
#[derive(Debug, Clone, Copy)]
pub struct ReadyQuery<'a> {
    pub workspace_id: Id,
    pub project: Option<Id>,
    pub repository: Option<Id>,
    /// Tasks unassigned or assigned to this agent.
    pub agent: Option<&'a str>,
    pub limit: usize,
}

fn priority_rank(priority: TaskPriority) -> u8 {
    match priority {
        TaskPriority::Critical => 0,
        TaskPriority::High => 1,
        TaskPriority::Medium => 2,
        TaskPriority::Low => 3,
    }
}

/// Ready tasks whose dependencies are all satisfied, by priority then age (then task number),
/// at most `limit`.
pub fn ready_tasks<'a>(
    tasks: &'a TaskSlice,
    stacks: &StackSlice,
    query: ReadyQuery<'_>,
) -> Vec<&'a Task> {
    let ordered: BTreeMap<(u8, wg_types::Timestamp, i64), &Task> = tasks
        .tasks
        .values()
        .filter(|task| {
            task.workspace_id == query.workspace_id
                && task.archived_at.is_none()
                && task.status == TaskStatus::Ready
                && query
                    .project
                    .is_none_or(|project| tasks.task_projects.contains_key(&(task.id, project)))
                && query.repository.is_none_or(|repository| {
                    tasks.task_repositories.contains_key(&(task.id, repository))
                })
                && query.agent.is_none_or(|agent| {
                    task.assigned_agent
                        .as_deref()
                        .is_none_or(|assigned| assigned == agent)
                })
        })
        .map(|task| {
            (
                (priority_rank(task.priority), task.created_at, task.number),
                task,
            )
        })
        .collect();
    ordered
        .into_values()
        .filter(|task| unsatisfied_dependencies(tasks, stacks, task.id).is_empty())
        .take(query.limit)
        .collect()
}
