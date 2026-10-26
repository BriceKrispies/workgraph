//! Reads for the stack family (SH-07): stacks, their repositories and their items.
//!
//! The statements are built by `wg_storecodec::stack_sql` as values; this module only runs them
//! through the session and decodes the rows with the table codecs. Writes go through the generic
//! [`Session::apply_write`] and [`Session::apply_decision`] like every other family.

use wg_model::entity::{Stack, StackItem, StackRepository};
use wg_storecodec::{Statement, TableRow, stack_sql};
use wg_types::{Id, WgError};

use crate::Session;

/// Runs `statement` and decodes the first row, if there is one.
fn one<T: TableRow>(
    session: &mut Session<'_>,
    statement: &Statement,
) -> Result<Option<T>, WgError> {
    session
        .rows(statement)?
        .first()
        .map(|values| T::from_raw(values))
        .transpose()
}

/// Runs `statement` and decodes every row, in the order the query returns them.
fn many<T: TableRow>(session: &mut Session<'_>, statement: &Statement) -> Result<Vec<T>, WgError> {
    session
        .rows(statement)?
        .iter()
        .map(|values| T::from_raw(values))
        .collect()
}

impl Session<'_> {
    /// The stack whose id is `stack_id`, if there is one.
    pub fn stack_by_id(&mut self, stack_id: Id) -> Result<Option<Stack>, WgError> {
        one(self, &stack_sql::stack_by_id(stack_id))
    }

    /// The active stack of `parent_task_id`, if it has one.
    pub fn active_stack_for_parent(
        &mut self,
        parent_task_id: Id,
    ) -> Result<Option<Stack>, WgError> {
        one(self, &stack_sql::active_stack_for_parent(parent_task_id))
    }

    /// The newest stack of `parent_task_id`, active or not.
    pub fn latest_stack_for_parent(
        &mut self,
        parent_task_id: Id,
    ) -> Result<Option<Stack>, WgError> {
        one(self, &stack_sql::latest_stack_for_parent(parent_task_id))
    }

    /// The stacks of `workspace_id`, newest first; landed and abandoned ones only with
    /// `include_inactive`.
    pub fn stacks_in_workspace(
        &mut self,
        workspace_id: Id,
        include_inactive: bool,
    ) -> Result<Vec<Stack>, WgError> {
        many(
            self,
            &stack_sql::stacks_in_workspace(workspace_id, include_inactive),
        )
    }

    /// Every stack that `task_id` is an item of, newest first, inactive ones included.
    pub fn stacks_containing_task(&mut self, task_id: Id) -> Result<Vec<Stack>, WgError> {
        many(self, &stack_sql::stacks_containing_task(task_id))
    }

    /// The item of `task_id` in its active stack, if it is an item of one.
    pub fn active_item_for_task(&mut self, task_id: Id) -> Result<Option<StackItem>, WgError> {
        one(self, &stack_sql::active_item_for_task(task_id))
    }

    /// Every item of `task_id` across stacks, newest stack first.
    pub fn items_for_task(&mut self, task_id: Id) -> Result<Vec<StackItem>, WgError> {
        many(self, &stack_sql::items_for_task(task_id))
    }

    /// The repository rows of `stack_id`, by repository name.
    pub fn stack_repositories(&mut self, stack_id: Id) -> Result<Vec<StackRepository>, WgError> {
        many(self, &stack_sql::stack_repositories(stack_id))
    }

    /// The repository row of `stack_id` for `repository_id`, if the stack covers it.
    pub fn stack_repository(
        &mut self,
        stack_id: Id,
        repository_id: Id,
    ) -> Result<Option<StackRepository>, WgError> {
        one(self, &stack_sql::stack_repository(stack_id, repository_id))
    }

    /// The repository row of the active stack that `task_id` belongs to, for `repository_id`.
    pub fn active_repository_for_task(
        &mut self,
        task_id: Id,
        repository_id: Id,
    ) -> Result<Option<StackRepository>, WgError> {
        one(
            self,
            &stack_sql::active_repository_for_task(task_id, repository_id),
        )
    }

    /// The items of `stack_id` in merge order (unordered items last, ties by task number).
    pub fn stack_items(&mut self, stack_id: Id) -> Result<Vec<StackItem>, WgError> {
        many(self, &stack_sql::stack_items(stack_id))
    }

    /// The item of `task_id` in `stack_id`, if the task is one.
    pub fn stack_item(&mut self, stack_id: Id, task_id: Id) -> Result<Option<StackItem>, WgError> {
        one(self, &stack_sql::stack_item(stack_id, task_id))
    }
}
