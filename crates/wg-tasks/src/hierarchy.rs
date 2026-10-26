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
    /// Count per status, for the statuses that occur, in the order each status is first met
    /// walking the descendants (the insertion order of Python's `by_status` dict).
    pub by_status: Vec<(TaskStatus, usize)>,
    pub accepted: usize,
    /// Accepted over non-cancelled descendants, rounded to four places; 1.0 when none count.
    pub progress: f64,
}

/// Every task below `task_id`, level by level. Within a level the children of each parent come
/// in parent id order, and siblings in creation order: the order Python reads them in, one
/// `parent_id IN (...)` query per level. Archived tasks count.
pub fn descendants(tasks: &TaskSlice, task_id: Id) -> Vec<&Task> {
    walk_down(tasks, &[task_id], BTreeSet::from([task_id]), Vec::new())
}

fn walk_down<'a>(
    tasks: &'a TaskSlice,
    frontier: &[Id],
    seen: BTreeSet<Id>,
    found: Vec<&'a Task>,
) -> Vec<&'a Task> {
    let level: Vec<&'a Task> = tasks
        .tasks
        .values()
        .filter(|task| {
            task.parent_id
                .is_some_and(|parent| frontier.contains(&parent))
                && !seen.contains(&task.id)
        })
        .map(|task| ((task.parent_id, task.number, task.id), task))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    if level.is_empty() {
        return found;
    }
    let level_ids: Vec<Id> = level.iter().map(|task| task.id).collect();
    let seen = seen.into_iter().chain(level_ids.iter().copied()).collect();
    let found = found.into_iter().chain(level.iter().copied()).collect();
    walk_down(tasks, &level_ids, seen, found)
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

/// The count of `status` in a by-status list, zero when it does not occur.
fn count_in(by_status: &[(TaskStatus, usize)], status: TaskStatus) -> usize {
    by_status
        .iter()
        .find(|(found, _)| *found == status)
        .map_or(0, |(_, count)| *count)
}

/// The rollup of the tasks below `task_id`, or `None` when it has no children.
pub fn rollup(tasks: &TaskSlice, task_id: Id) -> Option<Rollup> {
    let children = descendants(tasks, task_id);
    if children.is_empty() {
        return None;
    }
    // First-seen order: a status joins the list where it first occurs, later ones only count.
    let by_status: Vec<(TaskStatus, usize)> =
        children
            .iter()
            .fold(Vec::new(), |acc: Vec<(TaskStatus, usize)>, child| {
                if count_in(&acc, child.status) == 0 {
                    acc.into_iter()
                        .chain(std::iter::once((child.status, 1)))
                        .collect()
                } else {
                    acc.into_iter()
                        .map(|(status, count)| {
                            if status == child.status {
                                (status, count + 1)
                            } else {
                                (status, count)
                            }
                        })
                        .collect()
                }
            });
    let accepted = count_in(&by_status, TaskStatus::Accepted);
    let countable = children.len() - count_in(&by_status, TaskStatus::Cancelled);
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
