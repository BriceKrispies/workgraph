//! The three stack lookups other features read (SH-07): a stack by id, the repository row a
//! child's attempt builds from, and a stack's item for a task. The rest of the stack readers belong to the stacks feature.
//!
//! The statements are built by `wg_storecodec::stack_sql` as values; this module only runs them
//! through the session and decodes the rows with the table codecs.

use wg_model::entity::{Stack, StackItem, StackRepository};
use wg_storecodec::stack_sql;
use wg_types::{Id, WgError};

use crate::Session;
use crate::query::one;

impl Session<'_> {
    /// The stack whose id is `stack_id`, if there is one.
    pub fn stack_by_id(&mut self, stack_id: Id) -> Result<Option<Stack>, WgError> {
        one(self, &stack_sql::stack_by_id(stack_id))
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

    /// The item of `task_id` in `stack_id`, if the task is one.
    pub fn stack_item(&mut self, stack_id: Id, task_id: Id) -> Result<Option<StackItem>, WgError> {
        one(self, &stack_sql::stack_item(stack_id, task_id))
    }
}
