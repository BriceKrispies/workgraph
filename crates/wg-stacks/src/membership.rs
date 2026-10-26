//! Stack membership rules: adding, removing, readmitting children. Mirrors `StackService.add_item`,
//! `remove_item` and `readmit_item` in `core/services/stacks.py`. The database lookups that feed
//! these checks (parent link, existing rows, repository links) stay in the shell.

use std::collections::BTreeSet;

use wg_types::{ErrorCode, Id, StackItemState, TaskStatus, WgError};

use crate::item::is_removable;

/// Repository names of the child's repositories that the stack does not cover, sorted by name.
///
/// `child_repos` and `stack_repos` are repository ids; `names` maps repository ids to names.
/// A child may link only repositories the stack already covers.
pub fn outside_repositories(
    child_repos: &[Id],
    stack_repos: &[Id],
    names: &[(Id, String)],
) -> Vec<String> {
    // Paired with the position so a repeated name is kept through the sort.
    names
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| child_repos.contains(id) && !stack_repos.contains(id))
        .map(|(index, (_, name))| (name.clone(), index))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// Checks adding a child to a stack, in the order the service checks it.
///
/// `is_child_of_parent` is whether the task's parent is the stack's parent task.
/// `outside` is the output of [`outside_repositories`].
pub fn check_add_item(
    task_key: &str,
    parent_key: &str,
    is_child_of_parent: bool,
    already_member: bool,
    outside: &[String],
) -> Result<(), WgError> {
    if !is_child_of_parent {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("{task_key} is not a child of {parent_key}"),
        )
        .with_detail("task", task_key.into()));
    }
    if already_member {
        return Err(WgError::new(
            ErrorCode::Conflict,
            format!("{task_key} is already in the stack"),
        )
        .with_detail("task", task_key.into()));
    }
    if !outside.is_empty() {
        let names: Vec<String> = outside.to_vec();
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("{task_key} links repositories that are not part of the stack"),
        )
        .with_detail("task", task_key.into())
        .with_detail("repositories", serde_json::json!(names)));
    }
    Ok(())
}

/// Checks removing an item in `state` from its stack.
pub fn check_remove_item(task_key: &str, state: StackItemState) -> Result<(), WgError> {
    if is_removable(state) {
        return Ok(());
    }
    Err(WgError::new(
        ErrorCode::Conflict,
        format!("{task_key} is {state} on the integration branch and cannot leave the stack"),
    )
    .with_detail("task", task_key.into())
    .with_detail("state", state.as_str().into()))
}

/// Checks a readmit of an item in `state`: only an ejected or pending child may be readmitted.
pub fn check_readmit(task_key: &str, state: StackItemState) -> Result<(), WgError> {
    if matches!(state, StackItemState::Ejected | StackItemState::Pending) {
        return Ok(());
    }
    Err(WgError::new(
        ErrorCode::Conflict,
        format!("{task_key} is {state}; only an ejected or pending child can be readmitted"),
    )
    .with_detail("task", task_key.into())
    .with_detail("state", state.as_str().into()))
}

/// The task statuses a readmit walks the task through.
///
/// An ejection returns the task to `ready`; readmitting it puts the task back to awaiting
/// verification, through `running` as the lifecycle requires. Any other status is left alone.
pub fn readmit_task_path(task_status: TaskStatus) -> &'static [TaskStatus] {
    match task_status {
        TaskStatus::Ready => &[TaskStatus::Running, TaskStatus::AwaitingVerification],
        _ => &[],
    }
}
