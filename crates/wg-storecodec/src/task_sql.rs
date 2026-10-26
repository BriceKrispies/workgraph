//! SQL text and parameters for the task family (SH-04): tasks, their files, tags, links, comments,
//! dependencies and relations, and the `task_fts` index.
//!
//! Like [`crate::sql`], every statement is a value ([`Statement`]); `wg-store` binds and runs it.
//! The FTS document is built here from already-loaded rows, mirroring `search/indexer.py`'s
//! `index_task`: the store loads the task, its comments, tags and current classification, and this
//! module decides what text the index holds.

use wg_model::Table;
use wg_model::entity::{Classification, Task, TaskComment, TaskTag};
use wg_types::{Id, TaskKey, TaskStatus};

use crate::codec::{Raw, id_sql, text_sql};
use crate::rows::columns;
use crate::sql::{Statement, quote};

/// `SELECT <columns of table> FROM <table> WHERE <clause> ORDER BY <order>`.
fn select(table: Table, clause: &str, order: &str, params: Vec<Raw>) -> Statement {
    let names: Vec<String> = columns(table).iter().map(|c| quote(c)).collect();
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE {clause} ORDER BY {order}",
            names.join(", "),
            quote(table.name()),
        ),
        params,
    }
}

/// The task with `key` in `workspace_id` (keys are unique per workspace).
pub fn task_by_key(workspace_id: Id, key: &TaskKey) -> Statement {
    select(
        Table::Task,
        "\"workspace_id\" = ?1 AND \"key\" = ?2",
        "\"number\"",
        vec![id_sql(workspace_id), Raw::Text(key.to_string())],
    )
}

/// The tasks of a workspace in number order, optionally only those in `status`.
pub fn tasks_in_workspace(workspace_id: Id, status: Option<TaskStatus>) -> Statement {
    match status {
        Some(status) => select(
            Table::Task,
            "\"workspace_id\" = ?1 AND \"status\" = ?2",
            "\"number\"",
            vec![id_sql(workspace_id), text_sql(status.as_str())],
        ),
        None => select(
            Table::Task,
            "\"workspace_id\" = ?1",
            "\"number\"",
            vec![id_sql(workspace_id)],
        ),
    }
}

/// The files attached to a task, in `position` order.
pub fn task_files(task_id: Id) -> Statement {
    select(
        Table::TaskFile,
        "\"task_id\" = ?1",
        "\"position\"",
        vec![id_sql(task_id)],
    )
}

/// The tags of a task, sorted by tag.
pub fn task_tags(task_id: Id) -> Statement {
    select(
        Table::TaskTag,
        "\"task_id\" = ?1",
        "\"tag\"",
        vec![id_sql(task_id)],
    )
}

/// The external links of a task, oldest first.
pub fn task_links(task_id: Id) -> Statement {
    select(
        Table::TaskLink,
        "\"task_id\" = ?1",
        "\"created_at\", \"id\"",
        vec![id_sql(task_id)],
    )
}

/// The comments of a task, oldest first (`task_comments` by `created_at`).
pub fn task_comments(task_id: Id) -> Statement {
    select(
        Table::TaskComment,
        "\"task_id\" = ?1",
        "\"created_at\", \"id\"",
        vec![id_sql(task_id)],
    )
}

/// The dependencies a task waits on: rows whose `task_id` is `task_id`.
pub fn task_dependencies(task_id: Id) -> Statement {
    select(
        Table::TaskDependency,
        "\"task_id\" = ?1",
        "\"created_at\", \"id\"",
        vec![id_sql(task_id)],
    )
}

/// The dependencies that wait on a task: rows whose `depends_on_id` is `task_id`.
pub fn task_dependents(task_id: Id) -> Statement {
    select(
        Table::TaskDependency,
        "\"depends_on_id\" = ?1",
        "\"created_at\", \"id\"",
        vec![id_sql(task_id)],
    )
}

/// The relations a task takes part in, from either end.
pub fn task_relations(task_id: Id) -> Statement {
    select(
        Table::TaskRelation,
        "\"source_task_id\" = ?1 OR \"target_task_id\" = ?1",
        "\"created_at\", \"id\"",
        vec![id_sql(task_id)],
    )
}

/// The current classification row, by its id.
pub fn classification_by_id(classification_id: Id) -> Statement {
    select(
        Table::Classification,
        "\"id\" = ?1",
        "\"id\"",
        vec![id_sql(classification_id)],
    )
}

/// The text the `task_fts` row for one task holds (the columns of `search/indexer.py`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskIndexDoc {
    pub task_id: Id,
    pub workspace_id: Id,
    pub key: String,
    pub title: String,
    pub description: String,
    /// Comment bodies, oldest first, one per line.
    pub comments: String,
    /// Tags, space separated.
    pub tags: String,
    /// The current classification's category, subtype, domain, affected systems and tags,
    /// space separated; empty values are left out.
    pub summary: String,
}

/// Builds the index document for a task from its loaded rows. `comments` are sorted by
/// `created_at` here (ties keep the given order); `tags` keep the given order; `classification` is
/// the task's current classification, if it has one.
pub fn task_index_doc(
    task: &Task,
    comments: &[TaskComment],
    tags: &[TaskTag],
    classification: Option<&Classification>,
) -> TaskIndexDoc {
    let mut ordered: Vec<&TaskComment> = comments.iter().collect();
    ordered.sort_by_key(|comment| comment.created_at);
    let comments = ordered
        .iter()
        .map(|comment| comment.body.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let tags = tags
        .iter()
        .map(|tag| tag.tag.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut parts: Vec<String> = Vec::new();
    if let Some(cls) = classification {
        parts.extend(
            [
                cls.category.clone(),
                cls.subtype.clone(),
                cls.domain.clone(),
            ]
            .into_iter()
            .flatten(),
        );
        parts.extend(cls.affected_systems.iter().cloned());
        parts.extend(cls.tags.iter().cloned());
    }
    parts.retain(|part| !part.is_empty());
    TaskIndexDoc {
        task_id: task.id,
        workspace_id: task.workspace_id,
        key: task.key.to_string(),
        title: task.title.clone(),
        description: task.description.clone(),
        comments,
        tags,
        summary: parts.join(" "),
    }
}

/// Removes a task's `task_fts` row (a no-op when it has none).
pub fn fts_delete(task_id: Id) -> Statement {
    Statement {
        sql: "DELETE FROM \"task_fts\" WHERE \"task_id\" = ?1".to_owned(),
        params: vec![id_sql(task_id)],
    }
}

/// Inserts the `task_fts` row for a document.
pub fn fts_insert(doc: &TaskIndexDoc) -> Statement {
    Statement {
        sql: "INSERT INTO \"task_fts\" (\"task_id\", \"workspace_id\", \"key\", \"title\", \
              \"description\", \"comments\", \"tags\", \"summary\") \
              VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            .to_owned(),
        params: vec![
            id_sql(doc.task_id),
            id_sql(doc.workspace_id),
            text_sql(&doc.key),
            text_sql(&doc.title),
            text_sql(&doc.description),
            text_sql(&doc.comments),
            text_sql(&doc.tags),
            text_sql(&doc.summary),
        ],
    }
}
