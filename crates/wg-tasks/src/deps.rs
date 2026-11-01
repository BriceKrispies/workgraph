//! The dependency graph of tasks (GV-19). Mirrors `satisfied_dependencies`,
//! `unsatisfied_dependencies`, `depends_on_reachable` and the cycle check of `add_dependency` in
//! `core/services/tasks.py`.
//!
//! A dependency is satisfied once the task it names is accepted or cancelled. Inside an active stack a
//! dependency on a sibling that is an item of the same stack is satisfied as soon as that item
//! is integration-ready (ready, merged or verified). Every dependency check goes through
//! [`satisfied_dependencies`].
//!
//! The functions read two slices: the tasks, and the stacks whose items decide the sibling rule.
//! Neighbours are visited in id order, which is the order SQLite returns for the unique index on
//! `(task_id, depends_on_id)`.

use std::collections::{BTreeMap, BTreeSet};

use wg_model::slice::StackSlice;
use wg_model::{Task, TaskSlice};
use wg_types::{Id, StackItemState, StackStatus, TaskStatus, WgError};

/// Metadata value of `blocked_reason` for a task parked by unsatisfied dependencies.
pub const BLOCKED_BY_DEPENDENCIES: &str = "dependencies";

/// Statuses that satisfy a dependency for automatic scheduling (ready/blocked recomputation,
/// promotion, the scheduler; `SCHEDULING_DEPENDENCY_STATUSES`). A cancelled task will
/// never be accepted, so waiting on it would park the dependent forever; cancelling a dependency
/// releases it the same way accepting one does.
pub const DEPENDENCY_SATISFIED_STATUSES: &[TaskStatus] =
    &[TaskStatus::Accepted, TaskStatus::Cancelled];

/// Statuses that satisfy a dependency for a manual claim (`CLAIMABLE_DEPENDENCY_STATUSES`): the
/// scheduling set plus blockers whose work is complete and only awaiting review or verification.
/// Acceptance of the dependent still needs every blocker accepted (see [`unaccepted_dependencies`]).
pub const CLAIMABLE_DEPENDENCY_STATUSES: &[TaskStatus] = &[
    TaskStatus::Accepted,
    TaskStatus::Cancelled,
    TaskStatus::AwaitingReview,
    TaskStatus::AwaitingVerification,
];

/// Item states that satisfy a sibling dependency inside a stack
/// (`STACK_DEPENDENCY_SATISFIED_STATES`).
pub const STACK_DEPENDENCY_SATISFIED_STATES: &[StackItemState] = &[
    StackItemState::Ready,
    StackItemState::Merged,
    StackItemState::Verified,
];

/// Stack statuses that are history (`INACTIVE_STACK_STATUSES`).
pub const INACTIVE_STACK_STATUSES: &[StackStatus] = &[StackStatus::Landed, StackStatus::Abandoned];

/// What adding a dependency edge does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyAdd {
    /// The edge exists already; nothing changes.
    Exists,
    /// The edge is new and creates no cycle.
    Add,
}

/// The state of `task_id`'s item in `stack_id`, if the stack has one.
fn item_state(stacks: &StackSlice, stack_id: Id, task_id: Id) -> Option<StackItemState> {
    stacks
        .items
        .values()
        .find(|item| item.stack_id == stack_id && item.task_id == task_id)
        .map(|item| item.state)
}

/// Whether the sibling rule satisfies `task_id -> dep_id`: the first active stack holding both
/// as items decides, by the state of the dependency's item.
fn satisfied_in_stack(stacks: &StackSlice, task_id: Id, dep_id: Id) -> bool {
    stacks
        .stacks
        .values()
        .filter(|stack| !INACTIVE_STACK_STATUSES.contains(&stack.status))
        .find(|stack| {
            item_state(stacks, stack.id, task_id).is_some()
                && item_state(stacks, stack.id, dep_id).is_some()
        })
        .and_then(|stack| item_state(stacks, stack.id, dep_id))
        .is_some_and(|state| STACK_DEPENDENCY_SATISFIED_STATES.contains(&state))
}

/// Which `(task_id, depends_on_id)` edges are satisfied for automatic scheduling.
pub fn satisfied_dependencies(
    tasks: &TaskSlice,
    stacks: &StackSlice,
    edges: &[(Id, Id)],
) -> BTreeSet<(Id, Id)> {
    satisfied_dependencies_in(tasks, stacks, edges, DEPENDENCY_SATISFIED_STATUSES)
}

/// Which edges are satisfied when a dependency's task status is one of `statuses`.
pub fn satisfied_dependencies_in(
    tasks: &TaskSlice,
    stacks: &StackSlice,
    edges: &[(Id, Id)],
    statuses: &[TaskStatus],
) -> BTreeSet<(Id, Id)> {
    edges
        .iter()
        .copied()
        .filter(|(task_id, dep_id)| {
            tasks
                .task(*dep_id)
                .is_some_and(|t| statuses.contains(&t.status))
                || satisfied_in_stack(stacks, *task_id, *dep_id)
        })
        .collect()
}

/// The ids a task depends on, in id order.
pub fn dependency_ids(tasks: &TaskSlice, task_id: Id) -> Vec<Id> {
    tasks
        .dependencies_of(task_id)
        .map(|row| row.depends_on_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The tasks `task_id` still waits for, by task number.
pub fn unsatisfied_dependencies<'a>(
    tasks: &'a TaskSlice,
    stacks: &StackSlice,
    task_id: Id,
) -> Vec<&'a Task> {
    unsatisfied_dependencies_in(tasks, stacks, task_id, DEPENDENCY_SATISFIED_STATUSES)
}

/// The tasks `task_id` still waits for when a dependency counts as satisfied in `statuses`.
pub fn unsatisfied_dependencies_in<'a>(
    tasks: &'a TaskSlice,
    stacks: &StackSlice,
    task_id: Id,
    statuses: &[TaskStatus],
) -> Vec<&'a Task> {
    let edges: Vec<(Id, Id)> = dependency_ids(tasks, task_id)
        .into_iter()
        .map(|dep| (task_id, dep))
        .collect();
    let satisfied = satisfied_dependencies_in(tasks, stacks, &edges, statuses);
    edges
        .iter()
        .filter(|edge| !satisfied.contains(edge))
        .filter_map(|(_, dep)| tasks.task(*dep))
        .map(|task| (task.number, task))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// The direct dependencies of `task_id` that are not accepted yet, by task number: the
/// acceptance gate (`unaccepted_dependencies`). Only an accepted task counts: no
/// cancelled or stack-sibling exception applies.
pub fn unaccepted_dependencies(tasks: &TaskSlice, task_id: Id) -> Vec<&Task> {
    dependency_ids(tasks, task_id)
        .into_iter()
        .filter_map(|dep| tasks.task(dep))
        .filter(|task| task.status != TaskStatus::Accepted)
        .map(|task| (task.number, task))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// A dependency path `start -> ... -> target` if one exists (breadth first, so the shortest).
pub fn depends_on_reachable(tasks: &TaskSlice, start: Id, target: Id) -> Option<Vec<Id>> {
    let edges: BTreeSet<(Id, Id)> = tasks
        .dependencies
        .values()
        .map(|row| (row.task_id, row.depends_on_id))
        .collect();
    reach(&edges, target, vec![start], BTreeMap::from([(start, None)]))
}

/// One breadth-first level: the frontier is searched for `target`, then expanded in order.
fn reach(
    edges: &BTreeSet<(Id, Id)>,
    target: Id,
    frontier: Vec<Id>,
    parents: BTreeMap<Id, Option<Id>>,
) -> Option<Vec<Id>> {
    if frontier.is_empty() {
        return None;
    }
    if frontier.contains(&target) {
        return Some(path_to(&parents, target));
    }
    let (next, parents) = frontier.iter().fold(
        (Vec::new(), parents),
        |(next, parents): (Vec<Id>, BTreeMap<Id, Option<Id>>), current| {
            edges
                .range((*current, Id::nil())..=(*current, Id::max()))
                .map(|(_, nxt)| *nxt)
                .fold((next, parents), |(next, parents), nxt| {
                    if parents.contains_key(&nxt) {
                        (next, parents)
                    } else {
                        (
                            next.into_iter().chain(std::iter::once(nxt)).collect(),
                            parents
                                .into_iter()
                                .chain(std::iter::once((nxt, Some(*current))))
                                .collect(),
                        )
                    }
                })
        },
    );
    reach(edges, target, next, parents)
}

/// The path from the search start to `node`, following the parent links backwards.
fn path_to(parents: &BTreeMap<Id, Option<Id>>, node: Id) -> Vec<Id> {
    match parents.get(&node).copied().flatten() {
        Some(previous) => path_to(parents, previous)
            .into_iter()
            .chain(std::iter::once(node))
            .collect(),
        None => vec![node],
    }
}

fn missing_task(id: Id) -> WgError {
    WgError::not_found(format!("task {id} not found"))
}

/// The cycle check of `add_dependency`: `task_id` will depend on `depends_on_id`.
pub fn check_add_dependency(
    tasks: &TaskSlice,
    task_id: Id,
    depends_on_id: Id,
) -> Result<DependencyAdd, WgError> {
    let task = tasks.task(task_id).ok_or_else(|| missing_task(task_id))?;
    let dep = tasks
        .task(depends_on_id)
        .ok_or_else(|| missing_task(depends_on_id))?;
    if dep.id == task.id {
        return Err(WgError::dependency_cycle("a task cannot depend on itself"));
    }
    if tasks
        .dependencies_of(task.id)
        .any(|row| row.depends_on_id == dep.id)
    {
        return Ok(DependencyAdd::Exists);
    }
    match depends_on_reachable(tasks, dep.id, task.id) {
        None => Ok(DependencyAdd::Add),
        Some(path) => {
            let keys: Vec<String> = std::iter::once(task.key.to_string())
                .chain(path.iter().map(|id| {
                    tasks
                        .task(*id)
                        .map_or_else(|| id.to_string(), |t| t.key.to_string())
                }))
                .collect();
            Err(WgError::dependency_cycle(format!(
                "adding {} -> {} would create a cycle: {}",
                task.key,
                dep.key,
                keys.join(" -> ")
            ))
            .detail("cycle", keys))
        }
    }
}
