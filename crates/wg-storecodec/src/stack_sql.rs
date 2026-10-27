//! SQL text for the two stack lookups other features read (the worktree recovery pass and the
//! preparation of an attempt's worktrees): a stack by id, and the stack repository row a child's
//! attempt builds from. Every other stack statement belongs to the stacks feature
//! (`wg-stacks-feature`).
//!
//! Every SELECT lists the table's columns in schema order (qualified by an alias where the query
//! joins), so its rows decode through the same `TableRow` codecs as a fetch by key.

use wg_model::Table;
use wg_types::Id;

use crate::codec::id_sql;
use crate::sql::{Statement, select_list};

/// The statuses a stack no longer takes part in (Python `INACTIVE_STACK_STATUSES`), as SQL text.
const INACTIVE: &str = "('landed', 'abandoned')";

/// The stack whose id is `stack_id`.
pub fn stack_by_id(stack_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stacks\" WHERE \"id\" = ?1",
            select_list(Table::Stack, None)
        ),
        params: vec![id_sql(stack_id)],
    }
}

/// The repository row of the active stack that `task_id` is an item of, for `repository_id`: the
/// base and integration branch a child's attempt builds from. Empty when the task is in no active
/// stack or that stack does not cover the repository.
pub fn active_repository_for_task(task_id: Id, repository_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stack_repositories\" AS sr \
             JOIN \"stack_items\" AS si ON si.\"stack_id\" = sr.\"stack_id\" \
             JOIN \"stacks\" AS s ON s.\"id\" = sr.\"stack_id\" \
             WHERE si.\"task_id\" = ?1 AND sr.\"repository_id\" = ?2 \
             AND s.\"status\" NOT IN {INACTIVE}",
            select_list(Table::StackRepository, Some("sr"))
        ),
        params: vec![id_sql(task_id), id_sql(repository_id)],
    }
}
