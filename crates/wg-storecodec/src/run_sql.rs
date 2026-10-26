//! SQL text and parameters for the run family (SH-06): agent attempts, revision manifests, managed
//! worktrees, resource leases, change sets and their items, and artifacts.
//!
//! As in [`crate::task_sql`] and [`crate::repo_sql`], every statement is a [`Statement`] value and
//! every SELECT lists the table's columns in schema order, so its rows decode through the same
//! `TableRow` codecs as a fetch by key. Lists have a fixed `ORDER BY` so results are deterministic.
//! A status filter given as an empty slice means "no filter", as the Python services' `if statuses:`
//! checks do.

use wg_model::Table;
use wg_model::entity::ManagedWorktree;
use wg_types::{AttemptStatus, ErrorCode, Id, Timestamp, WgError, WorktreeStatus};

use crate::codec::{Cols, Raw, id_sql, text_sql, ts_sql};
use crate::rows::{TableRow, columns};
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

/// The `WHERE` clause and parameters of the attempts of `workspace_id` narrowed to one task, to
/// `statuses` and to one `provider`, each when given. Shared by the list and the count, so the
/// total is over exactly the rows the list pages through.
fn attempt_filter(
    workspace_id: Id,
    task_id: Option<Id>,
    statuses: &[AttemptStatus],
    provider: Option<&str>,
) -> (String, Vec<Raw>) {
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
    (clause, params)
}

/// The attempts of `workspace_id`, newest first (`created_at`, then `number`), narrowed as
/// [`attempt_filter`] describes.
pub fn attempts_in_workspace(
    workspace_id: Id,
    task_id: Option<Id>,
    statuses: &[AttemptStatus],
    provider: Option<&str>,
) -> Statement {
    let (clause, params) = attempt_filter(workspace_id, task_id, statuses, provider);
    select(
        Table::AgentAttempt,
        Some(&clause),
        "\"created_at\" DESC, \"number\" DESC",
        params,
    )
}

/// One page of [`attempts_in_workspace`]: `LIMIT limit OFFSET offset` after the same order.
pub fn attempts_in_workspace_page(
    workspace_id: Id,
    task_id: Option<Id>,
    statuses: &[AttemptStatus],
    provider: Option<&str>,
    limit: i64,
    offset: i64,
) -> Statement {
    let mut statement = attempts_in_workspace(workspace_id, task_id, statuses, provider);
    let first = statement.params.len() + 1;
    statement
        .sql
        .push_str(&format!(" LIMIT ?{first} OFFSET ?{}", first + 1));
    statement.params.push(Raw::Int(limit));
    statement.params.push(Raw::Int(offset));
    statement
}

/// The number of rows [`attempts_in_workspace`] lists for the same arguments (the page total).
pub fn attempts_in_workspace_count(
    workspace_id: Id,
    task_id: Option<Id>,
    statuses: &[AttemptStatus],
    provider: Option<&str>,
) -> Statement {
    let (clause, params) = attempt_filter(workspace_id, task_id, statuses, provider);
    Statement {
        sql: format!(
            "SELECT COUNT(*) FROM {} WHERE {clause}",
            quote(Table::AgentAttempt.name())
        ),
        params,
    }
}

/// The offset of the page after `offset` and `limit`, or `None` when the page reaches the total:
/// `offset + limit` when it is below `total`, as the Python attempts endpoint computes it.
pub fn next_offset(offset: i64, limit: i64, total: i64) -> Option<i64> {
    let after = offset + limit;
    (after < total).then_some(after)
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

/// The head a task's latest succeeded attempt recorded for one repository, with the branch it
/// pushed (`stacks.py`'s `latest_recorded`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedHead {
    pub head_sha: String,
    pub branch: Option<String>,
}

/// Decodes a [`latest_recorded_head`] row: `head_sha`, then `branch`.
pub fn decode_recorded_head(values: &[Raw]) -> Result<RecordedHead, WgError> {
    static NAMES: [&str; 2] = ["head_sha", "branch"];
    let cols = Cols::new(&NAMES, values);
    Ok(RecordedHead {
        head_sha: cols.text("head_sha")?,
        branch: cols.opt_text("branch")?,
    })
}

/// The newest succeeded attempt's recorded head of `task_id` in `repository_id`: the change-set
/// item of the newest change set (then the newest attempt number) that has a head SHA. Selects
/// `head_sha` and `branch`, decoded by [`decode_recorded_head`].
pub fn latest_recorded_head(task_id: Id, repository_id: Id) -> Statement {
    Statement {
        sql: "SELECT i.\"head_sha\", i.\"branch\" FROM \"change_set_items\" AS i \
              JOIN \"change_sets\" AS c ON c.\"id\" = i.\"change_set_id\" \
              JOIN \"agent_attempts\" AS a ON a.\"id\" = c.\"attempt_id\" \
              WHERE c.\"task_id\" = ?1 AND i.\"repository_id\" = ?2 \
              AND i.\"head_sha\" IS NOT NULL AND a.\"status\" = ?3 \
              ORDER BY c.\"created_at\" DESC, a.\"number\" DESC LIMIT 1"
            .to_owned(),
        params: vec![
            id_sql(task_id),
            id_sql(repository_id),
            text_sql(AttemptStatus::Succeeded.as_str()),
        ],
    }
}

/// A managed worktree with the name of its repository (`worktrees.py`'s worktree listing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeWithRepository {
    pub worktree: ManagedWorktree,
    pub repository_name: String,
}

/// Decodes every row of `rows` with `decode`, in order.
pub fn decode_rows<T>(
    rows: &[Vec<Raw>],
    decode: impl Fn(&[Raw]) -> Result<T, WgError>,
) -> Result<Vec<T>, WgError> {
    rows.iter().map(|values| decode(values)).collect()
}

/// Decodes a [`worktrees_with_repository`] row: the worktree's columns in schema order, then
/// `repository_name`.
pub fn decode_worktree_with_repository(values: &[Raw]) -> Result<WorktreeWithRepository, WgError> {
    static NAMES: [&str; 1] = ["repository_name"];
    let width = columns(Table::ManagedWorktree).len();
    if values.len() != width + 1 {
        return Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!(
                "worktree row has {} columns, expected {}",
                values.len(),
                width + 1
            ),
        ));
    }
    let worktree = ManagedWorktree::from_raw(&values[..width])?;
    let repository_name = Cols::new(&NAMES, &values[width..]).text("repository_name")?;
    Ok(WorktreeWithRepository {
        worktree,
        repository_name,
    })
}

/// Every managed worktree, or those of `workspace_id` and of `status` when given, newest first,
/// each with its repository's name. Decoded by [`decode_worktree_with_repository`].
pub fn worktrees_with_repository(
    workspace_id: Option<Id>,
    status: Option<WorktreeStatus>,
) -> Statement {
    let mut params = Vec::new();
    let mut clause = String::from("1 = 1");
    if let Some(workspace_id) = workspace_id {
        params.push(id_sql(workspace_id));
        clause.push_str(&format!(" AND w.\"workspace_id\" = ?{}", params.len()));
    }
    if let Some(status) = status {
        params.push(text_sql(status.as_str()));
        clause.push_str(&format!(" AND w.\"status\" = ?{}", params.len()));
    }
    let names: Vec<String> = columns(Table::ManagedWorktree)
        .iter()
        .map(|c| format!("w.\"{c}\""))
        .collect();
    Statement {
        sql: format!(
            "SELECT {}, r.\"name\" FROM \"managed_worktrees\" AS w \
             JOIN \"repositories\" AS r ON r.\"id\" = w.\"repository_id\" \
             WHERE {clause} ORDER BY w.\"created_at\" DESC, w.\"id\"",
            names.join(", ")
        ),
        params,
    }
}

/// The resource leases whose expiry is at or before `now`, by resource key. A lease with no
/// expiry never expires here.
pub fn expired_leases(now: Timestamp) -> Statement {
    select(
        Table::ResourceLease,
        Some("\"expires_at\" IS NOT NULL AND \"expires_at\" <= ?1"),
        "\"resource_key\"",
        vec![ts_sql(now)],
    )
}

/// Deletes the resource leases [`expired_leases`] selects for the same `now`.
pub fn delete_expired_leases(now: Timestamp) -> Statement {
    Statement {
        sql: format!(
            "DELETE FROM {} WHERE \"expires_at\" IS NOT NULL AND \"expires_at\" <= ?1",
            quote(Table::ResourceLease.name())
        ),
        params: vec![ts_sql(now)],
    }
}
