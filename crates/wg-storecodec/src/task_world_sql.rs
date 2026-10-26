//! SQL for loading the task world (IF-05): the rows a task command reads, as statements.
//!
//! A task command resolves references, filters, derives dependency state and rolls up children,
//! so it works on the whole workspace: the registry rows, every task with its tags, files,
//! links, dependencies, relations, project and repository memberships and tracker items, and the
//! stacks that decide the sibling-dependency rule. Comments are loaded for the tasks asked for,
//! because they are the large table and only a task's detail shows them.
//!
//! [`statements`] lists one statement per table with the table it fills. `wg-store` runs them
//! and decodes each row through that table's codec.

use wg_model::entity::{
    ExternalItem, Project, Repository, Stack, StackItem, Task, TaskComment, TaskDependency,
    TaskFile, TaskLink, TaskProject, TaskRelation, TaskRepository, TaskTag, Workspace,
};
use wg_model::{Row, Table};
use wg_types::{ErrorCode, Id, WgError};

use crate::codec::{Raw, id_sql};
use crate::rows::TableRow;
use crate::rows::columns;
use crate::sql::Statement;

/// `SELECT <columns> FROM <table> [WHERE <filter>] ORDER BY <order>`.
fn select(table: Table, filter: Option<&str>, order: &str, params: Vec<Raw>) -> Statement {
    let names: Vec<String> = columns(table).iter().map(|c| format!("\"{c}\"")).collect();
    let clause = filter.map_or_else(String::new, |f| format!(" WHERE {f}"));
    Statement {
        sql: format!(
            "SELECT {} FROM \"{}\"{clause} ORDER BY {order}",
            names.join(", "),
            table.name()
        ),
        params,
    }
}

/// A statement over a table scoped by its own `workspace_id` column.
fn in_workspace(table: Table, workspace: Option<Id>, order: &str) -> Statement {
    match workspace {
        Some(id) => select(
            table,
            Some("\"workspace_id\" = ?1"),
            order,
            vec![id_sql(id)],
        ),
        None => select(table, None, order, Vec::new()),
    }
}

/// A statement over a table scoped through the task its `column` names.
fn of_tasks(table: Table, column: &str, workspace: Option<Id>, order: &str) -> Statement {
    match workspace {
        Some(id) => select(
            table,
            Some(&format!(
                "\"{column}\" IN (SELECT \"id\" FROM \"tasks\" WHERE \"workspace_id\" = ?1)"
            )),
            order,
            vec![id_sql(id)],
        ),
        None => select(table, None, order, Vec::new()),
    }
}

/// The comments of the given tasks, oldest first. No statement when `task_ids` is empty.
fn comments_of(task_ids: &[Id]) -> Option<Statement> {
    if task_ids.is_empty() {
        return None;
    }
    let marks: Vec<String> = (1..=task_ids.len()).map(|n| format!("?{n}")).collect();
    Some(select(
        Table::TaskComment,
        Some(&format!("\"task_id\" IN ({})", marks.join(", "))),
        "\"created_at\", \"id\"",
        task_ids.iter().copied().map(id_sql).collect(),
    ))
}

/// The items of the stacks of `workspace`.
fn stack_items(workspace: Option<Id>) -> Statement {
    match workspace {
        Some(id) => select(
            Table::StackItem,
            Some("\"stack_id\" IN (SELECT \"id\" FROM \"stacks\" WHERE \"workspace_id\" = ?1)"),
            "\"id\"",
            vec![id_sql(id)],
        ),
        None => select(Table::StackItem, None, "\"id\"", Vec::new()),
    }
}

/// Every statement of the task world for `workspace` (`None`: all workspaces), each with the
/// table whose rows it returns, and the comments of `comments_for`.
#[must_use]
pub fn statements(workspace: Option<Id>, comments_for: &[Id]) -> Vec<(Table, Statement)> {
    let mut out = vec![
        (
            Table::Workspace,
            select(Table::Workspace, None, "\"id\"", Vec::new()),
        ),
        (
            Table::Project,
            in_workspace(Table::Project, workspace, "\"id\""),
        ),
        (
            Table::Repository,
            in_workspace(Table::Repository, workspace, "\"id\""),
        ),
        (
            Table::Task,
            in_workspace(Table::Task, workspace, "\"number\", \"id\""),
        ),
        (
            Table::TaskFile,
            of_tasks(
                Table::TaskFile,
                "task_id",
                workspace,
                "\"task_id\", \"position\"",
            ),
        ),
        (
            Table::TaskTag,
            of_tasks(Table::TaskTag, "task_id", workspace, "\"task_id\", \"tag\""),
        ),
        (
            Table::TaskLink,
            of_tasks(Table::TaskLink, "task_id", workspace, "\"id\""),
        ),
        (
            Table::TaskDependency,
            of_tasks(Table::TaskDependency, "task_id", workspace, "\"id\""),
        ),
        (
            Table::TaskRelation,
            of_tasks(Table::TaskRelation, "source_task_id", workspace, "\"id\""),
        ),
        (
            Table::TaskProject,
            of_tasks(
                Table::TaskProject,
                "task_id",
                workspace,
                "\"task_id\", \"project_id\"",
            ),
        ),
        (
            Table::TaskRepository,
            of_tasks(
                Table::TaskRepository,
                "task_id",
                workspace,
                "\"task_id\", \"repository_id\"",
            ),
        ),
        (
            Table::ExternalItem,
            in_workspace(Table::ExternalItem, workspace, "\"id\""),
        ),
        (
            Table::Stack,
            in_workspace(Table::Stack, workspace, "\"id\""),
        ),
        (Table::StackItem, stack_items(workspace)),
    ];
    out.extend(comments_of(comments_for).map(|statement| (Table::TaskComment, statement)));
    out
}

/// The ids of the tasks whose full-text row matches `match_expr`.
#[must_use]
pub fn fts_task_ids(match_expr: &str) -> Statement {
    Statement {
        sql: "SELECT \"task_id\" FROM \"task_fts\" WHERE \"task_fts\" MATCH ?1".to_owned(),
        params: vec![Raw::Text(match_expr.to_owned())],
    }
}

/// The rows [`statements`] loads, as the two slices the task decisions read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskWorld {
    pub tasks: wg_model::TaskSlice,
    pub stacks: wg_model::slice::StackSlice,
}

impl TaskWorld {
    /// The world with `rows` added: stack rows to the stack slice, every other row to the task
    /// slice (which keeps the registry rows too).
    #[must_use]
    pub fn with_rows(self, rows: impl IntoIterator<Item = wg_model::Row>) -> Self {
        rows.into_iter().fold(self, |world, row| {
            let write = wg_model::Write::insert(row);
            TaskWorld {
                tasks: world.tasks.apply(&write),
                stacks: world.stacks.apply(&write),
            }
        })
    }
}

/// Decodes one result row of `table` into a [`Row`].
pub fn decode_row(table: Table, values: &[Raw]) -> Result<Row, WgError> {
    macro_rules! by_table {
        ($($variant:ident),+ $(,)?) => {
            match table {
                $(Table::$variant => Ok(Row::from($variant::from_raw(values)?)),)+
                other => Err(WgError::new(
                    ErrorCode::WorkgraphError,
                    format!("the task world does not load {}", other.name()),
                )),
            }
        };
    }
    by_table!(
        Workspace,
        Project,
        Repository,
        Task,
        TaskFile,
        TaskTag,
        TaskLink,
        TaskComment,
        TaskDependency,
        TaskRelation,
        TaskProject,
        TaskRepository,
        ExternalItem,
        Stack,
        StackItem,
    )
}
