//! The parent and child hierarchy of tasks (GV-19): descendants, ancestry, the cycle check of a
//! parent change and the progress rollup. Mirrors `descendants`, `is_ancestor` and `_rollup` in
//! `core/services/tasks.py`.

use std::collections::{BTreeMap, BTreeSet};

use wg_model::{Task, TaskSlice};
use wg_types::{ErrorCode, Id, TaskStatus, WgError};

/// Progress of the descendants of a task (`TaskRollup`).
#[derive(Debug, Clone, PartialEq)]
pub struct Rollup {
    pub total_descendants: usize,
    /// Count per status, for the statuses that occur.
    pub by_status: BTreeMap<TaskStatus, usize>,
    pub accepted: usize,
    /// Accepted over non-cancelled descendants, rounded to four places; 1.0 when none count.
    pub progress: f64,
}

/// Every task below `task_id`, level by level (siblings in id order). Archived tasks count.
pub fn descendants(tasks: &TaskSlice, task_id: Id) -> Vec<&Task> {
    let children_of: BTreeMap<Id, BTreeSet<Id>> =
        tasks
            .tasks
            .values()
            .fold(BTreeMap::new(), |acc, task| match task.parent_id {
                Some(parent) => {
                    let siblings: BTreeSet<Id> = acc
                        .get(&parent)
                        .into_iter()
                        .flatten()
                        .copied()
                        .chain(std::iter::once(task.id))
                        .collect();
                    acc.into_iter()
                        .chain(std::iter::once((parent, siblings)))
                        .collect()
                }
                None => acc,
            });
    walk_down(
        tasks,
        &children_of,
        vec![task_id],
        BTreeSet::from([task_id]),
        Vec::new(),
    )
}

fn walk_down<'a>(
    tasks: &'a TaskSlice,
    children_of: &BTreeMap<Id, BTreeSet<Id>>,
    frontier: Vec<Id>,
    seen: BTreeSet<Id>,
    found: Vec<&'a Task>,
) -> Vec<&'a Task> {
    let level: BTreeSet<Id> = frontier
        .iter()
        .flat_map(|id| children_of.get(id).into_iter().flatten().copied())
        .filter(|id| !seen.contains(id))
        .collect();
    if level.is_empty() {
        return found;
    }
    let seen = seen.into_iter().chain(level.iter().copied()).collect();
    let found = found
        .into_iter()
        .chain(level.iter().filter_map(|id| tasks.task(*id)))
        .collect();
    walk_down(tasks, children_of, level.into_iter().collect(), seen, found)
}

/// Whether `candidate_ancestor` is `task_id` or one of its ancestors. A loop in the parent
/// column ends the walk.
pub fn is_ancestor(tasks: &TaskSlice, candidate_ancestor: Id, task_id: Id) -> bool {
    walk_up(tasks, candidate_ancestor, Some(task_id), BTreeSet::new())
}

fn walk_up(tasks: &TaskSlice, candidate: Id, current: Option<Id>, seen: BTreeSet<Id>) -> bool {
    match current {
        Some(id) if !seen.contains(&id) => {
            id == candidate
                || walk_up(
                    tasks,
                    candidate,
                    tasks.task(id).and_then(|t| t.parent_id),
                    seen.into_iter().chain(std::iter::once(id)).collect(),
                )
        }
        _ => false,
    }
}

/// The cycle check of a parent change: `parent_id` may not be the task or one of its
/// descendants.
pub fn check_set_parent(tasks: &TaskSlice, task_id: Id, parent_id: Id) -> Result<(), WgError> {
    let missing = |id: Id| WgError::new(ErrorCode::NotFound, format!("task {id} not found"));
    let task = tasks.task(task_id).ok_or_else(|| missing(task_id))?;
    let parent = tasks.task(parent_id).ok_or_else(|| missing(parent_id))?;
    if parent.id == task.id || is_ancestor(tasks, task.id, parent.id) {
        return Err(WgError::new(
            ErrorCode::DependencyCycle,
            format!(
                "setting {} as parent of {} would create a hierarchy cycle",
                parent.key, task.key
            ),
        ));
    }
    Ok(())
}

/// Rounds to four places the way Python's `round(x, 4)` does: the exact binary value, ties to even.
fn round4(value: f64) -> f64 {
    format!("{value:.4}").parse().unwrap_or(value)
}

/// The rollup of the tasks below `task_id`, or `None` when it has no children.
pub fn rollup(tasks: &TaskSlice, task_id: Id) -> Option<Rollup> {
    let children = descendants(tasks, task_id);
    if children.is_empty() {
        return None;
    }
    let by_status = children.iter().fold(
        BTreeMap::new(),
        |acc: BTreeMap<TaskStatus, usize>, child| {
            let count = acc.get(&child.status).copied().unwrap_or(0) + 1;
            acc.into_iter()
                .chain(std::iter::once((child.status, count)))
                .collect()
        },
    );
    let count_of = |status| by_status.get(&status).copied().unwrap_or(0);
    let accepted = count_of(TaskStatus::Accepted);
    let countable = children.len() - count_of(TaskStatus::Cancelled);
    Some(Rollup {
        total_descendants: children.len(),
        accepted,
        progress: if countable == 0 {
            1.0
        } else {
            round4(accepted as f64 / countable as f64)
        },
        by_status,
    })
}
