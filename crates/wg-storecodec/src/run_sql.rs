//! SQL text and parameters for the run family (SH-06): agent attempts, revision manifests, managed
//! worktrees, resource leases, change sets and their items, and artifacts.
//!
//! As in [`crate::task_sql`] and [`crate::repo_sql`], every statement is a [`Statement`] value and
//! every SELECT lists the table's columns in schema order, so its rows decode through the same
//! `TableRow` codecs as a fetch by key. Lists have a fixed `ORDER BY` so results are deterministic.
//! A status filter given as an empty slice means "no filter", as the Python services' `if statuses:`
//! checks do.

use wg_model::Table;
use wg_types::{AttemptStatus, Id, Timestamp, WorktreeStatus};

use crate::codec::{Raw, id_sql, text_sql, ts_sql};
use crate::rows::columns;
use crate::sql::Statement;

/// Quotes an identifier.
fn quote(name: &str) -> String {
    format!("\"{name}\"")
}

/// `SELECT <columns of table> FROM <table> WHERE <clause> ORDER BY <order>`, or without the
/// `WHERE` when `clause` is `None`.
fn select(table: Table, clause: Option<&str>, order: &str, params: Vec<Raw>) -> Statement {
    let names: Vec<String> = columns(table).iter().map(|c| quote(c)).collect();
    let filter = clause.map(|c| format!(" WHERE {c}")).unwrap_or_default();
    Statement {
        sql: format!(
            "SELECT {} FROM {}{filter} ORDER BY {order}",
            names.join(", "),
            quote(table.name()),
        ),
        params,
    }
}

/// Appends ` AND "column" IN (?n, ...)` for `values` to `clause`, binding the values after
/// `params`. The caller passes a non-empty `values`.
fn push_in(clause: &mut String, params: &mut Vec<Raw>, column: &str, values: Vec<Raw>) {
    let first = params.len() + 1;
    let marks: Vec<String> = (first..first + values.len())
        .map(|n| format!("?{n}"))
        .collect();
    clause.push_str(&format!(" AND {} IN ({})", quote(column), marks.join(", ")));
    params.extend(values);
}

/// The attempt with `id`.
pub fn attempt_by_id(id: Id) -> Statement {
    select(
        Table::AgentAttempt,
        Some("\"id\" = ?1"),
        "\"id\"",
        vec![id_sql(id)],
    )
}

/// The attempts of `workspace_id`, newest first (`created_at`, then `number`), narrowed to one
/// task, to `statuses` and to one `provider`, each when given.
pub fn attempts_in_workspace(
    workspace_id: Id,
    task_id: Option<Id>,
    statuses: &[AttemptStatus],
    provider: Option<&str>,
) -> Statement {
    let mut params = vec![id_sql(workspace_id)];
    let mut clause = format!("{} = ?1", quote("workspace_id"));
    if let Some(task_id) = task_id {
        push_in(&mut clause, &mut params, "task_id", vec![id_sql(task_id)]);
    }
    if !statuses.is_empty() {
        let values = statuses.iter().map(|s| text_sql(s.as_str())).collect();
        push_in(&mut clause, &mut params, "status", values);
    }
    if let Some(provider) = provider {
        push_in(
            &mut clause,
            &mut params,
            "provider",
            vec![text_sql(provider)],
        );
    }
    select(
        Table::AgentAttempt,
        Some(&clause),
        "\"created_at\" DESC, \"number\" DESC",
        params,
    )
}

/// The attempts of `task_id`, by number (oldest first), narrowed to `statuses` when given.
pub fn attempts_of_task(task_id: Id, statuses: &[AttemptStatus]) -> Statement {
    let mut params = vec![id_sql(task_id)];
    let mut clause = format!("{} = ?1", quote("task_id"));
    if !statuses.is_empty() {
        let values = statuses.iter().map(|s| text_sql(s.as_str())).collect();
        push_in(&mut clause, &mut params, "status", values);
    }
    select(Table::AgentAttempt, Some(&clause), "\"number\"", params)
}

/// The revision manifest with `id`.
pub fn manifest_by_id(id: Id) -> Statement {
    select(
        Table::RevisionManifest,
        Some("\"id\" = ?1"),
        "\"id\"",
        vec![id_sql(id)],
    )
}

/// The revision manifests of `attempt_id`, oldest first.
pub fn manifests_of_attempt(attempt_id: Id) -> Statement {
    select(
        Table::RevisionManifest,
        Some("\"attempt_id\" = ?1"),
        "\"created_at\", \"id\"",
        vec![id_sql(attempt_id)],
    )
}

/// The managed worktree at `path` (paths are unique).
pub fn worktree_by_path(path: &str) -> Statement {
    select(
        Table::ManagedWorktree,
        Some("\"path\" = ?1"),
        "\"path\"",
        vec![text_sql(path)],
    )
}

/// The managed worktrees of `attempt_id` by path, narrowed to `statuses` when given.
pub fn worktrees_of_attempt(attempt_id: Id, statuses: &[WorktreeStatus]) -> Statement {
    let mut params = vec![id_sql(attempt_id)];
    let mut clause = format!("{} = ?1", quote("attempt_id"));
    if !statuses.is_empty() {
        let values = statuses.iter().map(|s| text_sql(s.as_str())).collect();
        push_in(&mut clause, &mut params, "status", values);
    }
    select(Table::ManagedWorktree, Some(&clause), "\"path\"", params)
}

/// The managed worktrees whose status is one of `statuses`, by path. An empty `statuses` lists
/// every worktree. Recovery scans use this to find the worktrees still in use.
pub fn worktrees_with_status(statuses: &[WorktreeStatus]) -> Statement {
    if statuses.is_empty() {
        return select(Table::ManagedWorktree, None, "\"path\"", Vec::new());
    }
    let mut params = Vec::new();
    let mut clause = String::from("1 = 1");
    let values = statuses.iter().map(|s| text_sql(s.as_str())).collect();
    push_in(&mut clause, &mut params, "status", values);
    select(Table::ManagedWorktree, Some(&clause), "\"path\"", params)
}

/// The resource lease on `resource_key`, if one is held.
pub fn lease_by_resource(resource_key: &str) -> Statement {
    select(
        Table::ResourceLease,
        Some("\"resource_key\" = ?1"),
        "\"resource_key\"",
        vec![text_sql(resource_key)],
    )
}

/// The resource leases held by `holder`, by resource key.
pub fn leases_held_by(holder: &str) -> Statement {
    select(
        Table::ResourceLease,
        Some("\"holder\" = ?1"),
        "\"resource_key\"",
        vec![text_sql(holder)],
    )
}

/// The change set with `id`.
pub fn change_set_by_id(id: Id) -> Statement {
    select(
        Table::ChangeSet,
        Some("\"id\" = ?1"),
        "\"id\"",
        vec![id_sql(id)],
    )
}

/// The change sets of `task_id`, oldest first.
pub fn change_sets_of_task(task_id: Id) -> Statement {
    select(
        Table::ChangeSet,
        Some("\"task_id\" = ?1"),
        "\"created_at\", \"id\"",
        vec![id_sql(task_id)],
    )
}

/// The change sets made by `attempt_id`, oldest first (an attempt has at most one in practice).
pub fn change_sets_of_attempt(attempt_id: Id) -> Statement {
    select(
        Table::ChangeSet,
        Some("\"attempt_id\" = ?1"),
        "\"created_at\", \"id\"",
        vec![id_sql(attempt_id)],
    )
}

/// The items of `change_set_id`, by repository id.
pub fn change_set_items(change_set_id: Id) -> Statement {
    select(
        Table::ChangeSetItem,
        Some("\"change_set_id\" = ?1"),
        "\"repository_id\"",
        vec![id_sql(change_set_id)],
    )
}

/// The artifact with `id`.
pub fn artifact_by_id(id: Id) -> Statement {
    select(
        Table::Artifact,
        Some("\"id\" = ?1"),
        "\"id\"",
        vec![id_sql(id)],
    )
}

/// The artifacts of `task_id`, oldest first.
pub fn artifacts_of_task(task_id: Id) -> Statement {
    select(
        Table::Artifact,
        Some("\"task_id\" = ?1"),
        "\"created_at\", \"id\"",
        vec![id_sql(task_id)],
    )
}

/// The artifacts of `attempt_id`, oldest first.
pub fn artifacts_of_attempt(attempt_id: Id) -> Statement {
    select(
        Table::Artifact,
        Some("\"attempt_id\" = ?1"),
        "\"created_at\", \"id\"",
        vec![id_sql(attempt_id)],
    )
}

/// The artifacts of `retention` created before `created_before` that have not been purged yet,
/// oldest first (the retention job's selection).
pub fn purgeable_artifacts(retention: &str, created_before: Timestamp) -> Statement {
    select(
        Table::Artifact,
        Some("\"retention\" = ?1 AND \"created_at\" < ?2 AND \"purged_at\" IS NULL"),
        "\"created_at\", \"id\"",
        vec![text_sql(retention), ts_sql(created_before)],
    )
}
