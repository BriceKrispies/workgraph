//! SQL text and parameters for the stack family (SH-07): stacks, their repositories and their
//! items. The lookups the stack services make by parent task, workspace, repository and task, and
//! the list a stack's detail shows.
//!
//! Every SELECT lists the table's [`columns`] in schema order (qualified by an alias where the
//! query joins), so its rows decode through the same `TableRow` codecs as a fetch by key. Lists
//! have a fixed `ORDER BY`, the one the Python services use, so results are deterministic.

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

/// The active stack of `parent_task_id`, if it has one (a unique index allows at most one stack
/// per parent that is neither landed nor abandoned).
pub fn active_stack_for_parent(parent_task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stacks\" WHERE \"parent_task_id\" = ?1 \
             AND \"status\" NOT IN {INACTIVE}",
            select_list(Table::Stack, None)
        ),
        params: vec![id_sql(parent_task_id)],
    }
}

/// The newest stack of `parent_task_id`, active or not.
pub fn latest_stack_for_parent(parent_task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stacks\" WHERE \"parent_task_id\" = ?1 \
             ORDER BY \"created_at\" DESC LIMIT 1",
            select_list(Table::Stack, None)
        ),
        params: vec![id_sql(parent_task_id)],
    }
}

/// The stacks of `workspace_id`, newest first. Unless `include_inactive`, landed and abandoned
/// stacks are left out.
pub fn stacks_in_workspace(workspace_id: Id, include_inactive: bool) -> Statement {
    let mut sql = format!(
        "SELECT {} FROM \"stacks\" WHERE \"workspace_id\" = ?1",
        select_list(Table::Stack, None)
    );
    if !include_inactive {
        sql.push_str(&format!(" AND \"status\" NOT IN {INACTIVE}"));
    }
    sql.push_str(" ORDER BY \"created_at\" DESC");
    Statement {
        sql,
        params: vec![id_sql(workspace_id)],
    }
}

/// Every stack that `task_id` is an item of, newest first, inactive ones included.
pub fn stacks_containing_task(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stacks\" AS s WHERE s.\"id\" IN \
             (SELECT \"stack_id\" FROM \"stack_items\" WHERE \"task_id\" = ?1) \
             ORDER BY s.\"created_at\" DESC",
            select_list(Table::Stack, Some("s"))
        ),
        params: vec![id_sql(task_id)],
    }
}

/// The item of `task_id` in its active stack, if it is an item of one.
pub fn active_item_for_task(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stack_items\" AS si JOIN \"stacks\" AS s \
             ON s.\"id\" = si.\"stack_id\" \
             WHERE si.\"task_id\" = ?1 AND s.\"status\" NOT IN {INACTIVE}",
            select_list(Table::StackItem, Some("si"))
        ),
        params: vec![id_sql(task_id)],
    }
}

/// Every item of `task_id` across stacks, newest stack first.
pub fn items_for_task(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stack_items\" AS si JOIN \"stacks\" AS s \
             ON s.\"id\" = si.\"stack_id\" \
             WHERE si.\"task_id\" = ?1 ORDER BY s.\"created_at\" DESC",
            select_list(Table::StackItem, Some("si"))
        ),
        params: vec![id_sql(task_id)],
    }
}

/// The repository rows of `stack_id`, ordered by the repository's name.
pub fn stack_repositories(stack_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stack_repositories\" AS sr JOIN \"repositories\" AS r \
             ON r.\"id\" = sr.\"repository_id\" WHERE sr.\"stack_id\" = ?1 ORDER BY r.\"name\"",
            select_list(Table::StackRepository, Some("sr"))
        ),
        params: vec![id_sql(stack_id)],
    }
}

/// The repository row of `stack_id` for `repository_id`, if the stack covers it.
pub fn stack_repository(stack_id: Id, repository_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stack_repositories\" \
             WHERE \"stack_id\" = ?1 AND \"repository_id\" = ?2",
            select_list(Table::StackRepository, None)
        ),
        params: vec![id_sql(stack_id), id_sql(repository_id)],
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

/// The items of `stack_id` in merge order: items with a merge order first, by that order, then the
/// unordered ones; ties by the task's number.
pub fn stack_items(stack_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stack_items\" AS si JOIN \"tasks\" AS t \
             ON t.\"id\" = si.\"task_id\" WHERE si.\"stack_id\" = ?1 \
             ORDER BY si.\"merge_order\" IS NULL, si.\"merge_order\", t.\"number\"",
            select_list(Table::StackItem, Some("si"))
        ),
        params: vec![id_sql(stack_id)],
    }
}

/// The item of `task_id` in `stack_id`, if the task is one.
pub fn stack_item(stack_id: Id, task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"stack_items\" WHERE \"stack_id\" = ?1 AND \"task_id\" = ?2",
            select_list(Table::StackItem, None)
        ),
        params: vec![id_sql(stack_id), id_sql(task_id)],
    }
}
