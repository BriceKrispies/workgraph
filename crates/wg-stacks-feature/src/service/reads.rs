//! Reads for the stack family (SH-07): stacks, their repositories and their items.
//!
//! The statements are built by [`crate::spec::sql`] as values; this module only runs them
//! through the session and decodes the rows with the table codecs. Writes go through the generic
//! [`Session::apply_write`] and [`Session::apply_decision`] like every other family.

use wg_model::entity::{Stack, StackItem, StackRepository};
use wg_store::Session;
use wg_store::query::{many, one};
use wg_types::{Id, WgError};

use crate::spec::sql as stack_sql;

/// The active stack of `parent_task_id`, if it has one.
pub fn active_stack_for_parent(
    session: &mut Session<'_>,
    parent_task_id: Id,
) -> Result<Option<Stack>, WgError> {
    one(session, &stack_sql::active_stack_for_parent(parent_task_id))
}

/// The newest stack of `parent_task_id`, active or not.
pub fn latest_stack_for_parent(
    session: &mut Session<'_>,
    parent_task_id: Id,
) -> Result<Option<Stack>, WgError> {
    one(session, &stack_sql::latest_stack_for_parent(parent_task_id))
}

/// The stacks of `workspace_id`, newest first; landed and abandoned ones only with
/// `include_inactive`.
pub fn stacks_in_workspace(
    session: &mut Session<'_>,
    workspace_id: Id,
    include_inactive: bool,
) -> Result<Vec<Stack>, WgError> {
    many(
        session,
        &stack_sql::stacks_in_workspace(workspace_id, include_inactive),
    )
}

/// Every stack that `task_id` is an item of, newest first, inactive ones included.
pub fn stacks_containing_task(
    session: &mut Session<'_>,
    task_id: Id,
) -> Result<Vec<Stack>, WgError> {
    many(session, &stack_sql::stacks_containing_task(task_id))
}

/// The item of `task_id` in its active stack, if it is an item of one.
pub fn active_item_for_task(
    session: &mut Session<'_>,
    task_id: Id,
) -> Result<Option<StackItem>, WgError> {
    one(session, &stack_sql::active_item_for_task(task_id))
}

/// Every item of `task_id` across stacks, newest stack first.
pub fn items_for_task(session: &mut Session<'_>, task_id: Id) -> Result<Vec<StackItem>, WgError> {
    many(session, &stack_sql::items_for_task(task_id))
}

/// The repository rows of `stack_id`, by repository name.
pub fn stack_repositories(
    session: &mut Session<'_>,
    stack_id: Id,
) -> Result<Vec<StackRepository>, WgError> {
    many(session, &stack_sql::stack_repositories(stack_id))
}

/// The repository row of `stack_id` for `repository_id`, if the stack covers it.
pub fn stack_repository(
    session: &mut Session<'_>,
    stack_id: Id,
    repository_id: Id,
) -> Result<Option<StackRepository>, WgError> {
    one(
        session,
        &stack_sql::stack_repository(stack_id, repository_id),
    )
}

/// The items of `stack_id` in merge order (unordered items last, ties by task number).
pub fn stack_items(session: &mut Session<'_>, stack_id: Id) -> Result<Vec<StackItem>, WgError> {
    many(session, &stack_sql::stack_items(stack_id))
}

/// The item of `task_id` in `stack_id`, if the task is one.
pub fn stack_item(
    session: &mut Session<'_>,
    stack_id: Id,
    task_id: Id,
) -> Result<Option<StackItem>, WgError> {
    one(session, &stack_sql::stack_item(stack_id, task_id))
}
