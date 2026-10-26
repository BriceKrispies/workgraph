//! SQL text and parameters for the git family (SH-09): commits and the rows that hang off them
//! (file changes, symbols, analyses, equivalences, task links), the object gaps a repository
//! records, and the change surfaces a task leaves in a repository.
//!
//! Like [`crate::task_sql`] and [`crate::repo_sql`], every statement is a value ([`Statement`]).
//! Each SELECT lists its table's [`columns`] in schema order, so its rows decode through the same
//! `TableRow` codecs as a fetch by key. A join appends the joined table's columns after the first
//! table's; the typed rows at the end of this module split such a combined row back into parts.
//! Lists have a fixed `ORDER BY` so results are deterministic.

use wg_model::Table;
use wg_model::entity::{Commit, CommitTaskLink};
use wg_types::{ErrorCode, Id, Timestamp, WgError};

use crate::codec::{Cols, Raw, bool_sql, id_sql, opt_ts_sql, text_sql};
use crate::rows::{TableRow, columns};
use crate::sql::{Statement, select_list};

/// `?first, ..., ?(first + count - 1)`, or `NULL` when there are none: an `IN (NULL)` list
/// matches nothing, which is what an empty id list should match.
fn placeholders(first: usize, count: usize) -> String {
    if count == 0 {
        return "NULL".to_owned();
    }
    (first..first + count)
        .map(|n| format!("?{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The ids as bound parameters, in order.
fn id_params(ids: &[Id]) -> Vec<Raw> {
    ids.iter().map(|id| id_sql(*id)).collect()
}

/// The commit of `repository_id` whose SHA is exactly `sha`.
pub fn commit_by_sha(repository_id: Id, sha: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commits\" WHERE \"repository_id\" = ?1 AND \"sha\" = ?2",
            select_list(Table::Commit, None)
        ),
        params: vec![id_sql(repository_id), text_sql(sha)],
    }
}

/// The commits of `repository_id` whose SHA starts with `prefix`, by SHA. `substr` rather than
/// `LIKE`, so no wildcard character in the prefix changes what it matches.
pub fn commits_by_sha_prefix(repository_id: Id, prefix: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commits\" WHERE \"repository_id\" = ?1 \
             AND substr(\"sha\", 1, length(?2)) = ?2 ORDER BY \"sha\"",
            select_list(Table::Commit, None)
        ),
        params: vec![id_sql(repository_id), text_sql(prefix)],
    }
}

/// Every SHA the repository has a commit row for, in SHA order (one text column).
pub fn commit_shas(repository_id: Id) -> Statement {
    Statement {
        sql: "SELECT \"sha\" FROM \"commits\" WHERE \"repository_id\" = ?1 ORDER BY \"sha\""
            .to_owned(),
        params: vec![id_sql(repository_id)],
    }
}

/// The commits with the given ids, by id.
pub fn commits_by_ids(ids: &[Id]) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commits\" WHERE \"id\" IN ({}) ORDER BY \"id\"",
            select_list(Table::Commit, None),
            placeholders(1, ids.len())
        ),
        params: id_params(ids),
    }
}

/// The WHERE clause and parameters of a repository's commit list: its commits, only reachable
/// ones when `reachable_only`, only commits that have a task link when `linked_only`.
fn repository_commit_filter(
    repository_id: Id,
    reachable_only: bool,
    linked_only: bool,
) -> (String, Vec<Raw>) {
    let mut clause = "\"repository_id\" = ?1".to_owned();
    let mut params = vec![id_sql(repository_id)];
    if reachable_only {
        params.push(bool_sql(true));
        clause.push_str(&format!(" AND \"reachable\" = ?{}", params.len()));
    }
    if linked_only {
        clause.push_str(" AND \"id\" IN (SELECT \"commit_id\" FROM \"commit_task_links\")");
    }
    (clause, params)
}

/// How many commits the repository's commit list holds under the same filter as
/// [`repository_commits_page`]. One row, one column.
pub fn repository_commits_count(
    repository_id: Id,
    reachable_only: bool,
    linked_only: bool,
) -> Statement {
    let (clause, params) = repository_commit_filter(repository_id, reachable_only, linked_only);
    Statement {
        sql: format!("SELECT COUNT(*) FROM \"commits\" WHERE {clause}"),
        params,
    }
}

/// One page of the repository's commit list, newest first (`committed_at` descending, then id).
pub fn repository_commits_page(
    repository_id: Id,
    reachable_only: bool,
    linked_only: bool,
    limit: i64,
    offset: i64,
) -> Statement {
    let (clause, mut params) = repository_commit_filter(repository_id, reachable_only, linked_only);
    let limit_at = params.len() + 1;
    params.push(Raw::Int(limit));
    params.push(Raw::Int(offset));
    Statement {
        sql: format!(
            "SELECT {} FROM \"commits\" WHERE {clause} \
             ORDER BY \"committed_at\" DESC, \"id\" LIMIT ?{limit_at} OFFSET ?{}",
            select_list(Table::Commit, None),
            limit_at + 1
        ),
        params,
    }
}

/// The task links of `commit_ids`, each with its task's key, by commit then link order.
pub fn links_with_task_keys(commit_ids: &[Id]) -> Statement {
    Statement {
        sql: format!(
            "SELECT {}, t.\"key\" FROM \"commit_task_links\" cl \
             JOIN \"tasks\" t ON t.id = cl.task_id \
             WHERE cl.commit_id IN ({}) ORDER BY cl.commit_id, cl.created_at, cl.id",
            select_list(Table::CommitTaskLink, Some("cl")),
            placeholders(1, commit_ids.len())
        ),
        params: id_params(commit_ids),
    }
}

/// The links between a commit and a task, oldest first; only those from `source` when given.
pub fn commit_task_links_between(commit_id: Id, task_id: Id, source: Option<&str>) -> Statement {
    let mut sql = format!(
        "SELECT {} FROM \"commit_task_links\" WHERE \"commit_id\" = ?1 AND \"task_id\" = ?2",
        select_list(Table::CommitTaskLink, None)
    );
    let mut params = vec![id_sql(commit_id), id_sql(task_id)];
    if let Some(source) = source {
        sql.push_str(" AND \"source\" = ?3");
        params.push(text_sql(source));
    }
    sql.push_str(" ORDER BY \"created_at\", \"id\"");
    Statement { sql, params }
}

/// The links of `task_id` at or above `min_confidence`, each with its commit and the name of the
/// commit's repository, ordered by commit date.
pub fn task_commit_links(task_id: Id, min_confidence: f64) -> Statement {
    Statement {
        sql: format!(
            "SELECT {}, {}, r.\"name\" FROM \"commit_task_links\" cl \
             JOIN \"commits\" c ON c.id = cl.commit_id \
             JOIN \"repositories\" r ON r.id = c.repository_id \
             WHERE cl.task_id = ?1 AND cl.confidence >= ?2 \
             ORDER BY c.\"committed_at\", c.id, cl.\"created_at\", cl.id",
            select_list(Table::CommitTaskLink, Some("cl")),
            select_list(Table::Commit, Some("c")),
        ),
        params: vec![id_sql(task_id), Raw::Real(min_confidence)],
    }
}

/// The file changes of `commit_ids`, by commit then path.
pub fn file_changes(commit_ids: &[Id]) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commit_file_changes\" WHERE \"commit_id\" IN ({}) \
             ORDER BY \"commit_id\", \"path\", \"id\"",
            select_list(Table::CommitFileChange, None),
            placeholders(1, commit_ids.len())
        ),
        params: id_params(commit_ids),
    }
}

/// For the file changes of `commit_ids`, grouped by path: how many touch the path and the lines
/// they add and remove. Columns `path`, `changes`, `insertions`, `deletions`; by path.
pub fn path_totals(commit_ids: &[Id]) -> Statement {
    Statement {
        sql: format!(
            "SELECT \"path\", COUNT(*), SUM(\"insertions\"), SUM(\"deletions\") \
             FROM \"commit_file_changes\" WHERE \"commit_id\" IN ({}) \
             GROUP BY \"path\" ORDER BY \"path\"",
            placeholders(1, commit_ids.len())
        ),
        params: id_params(commit_ids),
    }
}

/// The symbols of `file_change_ids`, by file change then id.
pub fn symbols(file_change_ids: &[Id]) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commit_symbols\" WHERE \"file_change_id\" IN ({}) \
             ORDER BY \"file_change_id\", \"id\"",
            select_list(Table::CommitSymbol, None),
            placeholders(1, file_change_ids.len())
        ),
        params: id_params(file_change_ids),
    }
}

/// The structure analyses of `commit_ids` (one per commit at most), by commit.
pub fn analyses(commit_ids: &[Id]) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commit_analyses\" WHERE \"commit_id\" IN ({}) ORDER BY \"commit_id\"",
            select_list(Table::CommitAnalysis, None),
            placeholders(1, commit_ids.len())
        ),
        params: id_params(commit_ids),
    }
}

/// The equivalences that start or end at `commit_id`, oldest first.
pub fn equivalences_of_commit(commit_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commit_equivalences\" \
             WHERE \"original_commit_id\" = ?1 OR \"equivalent_commit_id\" = ?1 \
             ORDER BY \"created_at\", \"id\"",
            select_list(Table::CommitEquivalence, None)
        ),
        params: vec![id_sql(commit_id)],
    }
}

/// The equivalence from `original` to `equivalent`, if one is recorded.
pub fn equivalence(original: Id, equivalent: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commit_equivalences\" \
             WHERE \"original_commit_id\" = ?1 AND \"equivalent_commit_id\" = ?2",
            select_list(Table::CommitEquivalence, None)
        ),
        params: vec![id_sql(original), id_sql(equivalent)],
    }
}

/// The commits of `repository_id` with patch id `patch_id` other than `excluding`, by SHA.
pub fn commits_with_patch_id(repository_id: Id, patch_id: &str, excluding: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commits\" WHERE \"repository_id\" = ?1 \
             AND \"patch_id\" = ?2 AND \"id\" != ?3 ORDER BY \"sha\"",
            select_list(Table::Commit, None)
        ),
        params: vec![id_sql(repository_id), text_sql(patch_id), id_sql(excluding)],
    }
}

/// The unreachable commits of `repository_id` with the same author email, author date and subject
/// as given, other than `excluding`, by SHA. A missing author date matches a missing date.
pub fn unreachable_commits_matching(
    repository_id: Id,
    author_email: &str,
    authored_at: Option<Timestamp>,
    subject: &str,
    excluding: Id,
) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"commits\" WHERE \"repository_id\" = ?1 AND \"id\" != ?2 \
             AND \"reachable\" = ?3 AND \"author_email\" = ?4 AND \"authored_at\" IS ?5 \
             AND \"subject\" = ?6 ORDER BY \"sha\"",
            select_list(Table::Commit, None)
        ),
        params: vec![
            id_sql(repository_id),
            id_sql(excluding),
            bool_sql(false),
            text_sql(author_email),
            opt_ts_sql(authored_at),
            text_sql(subject),
        ],
    }
}

/// The object gap of `repository_id` for `sha`, if one is recorded.
pub fn git_object_gap(repository_id: Id, sha: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"git_object_gaps\" WHERE \"repository_id\" = ?1 AND \"sha\" = ?2",
            select_list(Table::GitObjectGap, None)
        ),
        params: vec![id_sql(repository_id), text_sql(sha)],
    }
}

/// Every object gap of `repository_id`, in detection order.
pub fn git_object_gaps(repository_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"git_object_gaps\" WHERE \"repository_id\" = ?1 \
             ORDER BY \"detected_at\", \"id\"",
            select_list(Table::GitObjectGap, None)
        ),
        params: vec![id_sql(repository_id)],
    }
}

/// Every commit of `repository_id` with its id, SHA and reachability, by SHA.
pub fn commit_reachability(repository_id: Id) -> Statement {
    Statement {
        sql: "SELECT \"id\", \"sha\", \"reachable\" FROM \"commits\" \
              WHERE \"repository_id\" = ?1 ORDER BY \"sha\""
            .to_owned(),
        params: vec![id_sql(repository_id)],
    }
}

/// The change surfaces of `task_id`, one per repository.
pub fn task_change_surfaces(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"task_change_surfaces\" WHERE \"task_id\" = ?1 \
             ORDER BY \"repository_id\", \"id\"",
            select_list(Table::TaskChangeSurface, None)
        ),
        params: vec![id_sql(task_id)],
    }
}

/// The change surface `task_id` leaves in `repository_id`, if one is recorded.
pub fn task_change_surface(task_id: Id, repository_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"task_change_surfaces\" \
             WHERE \"task_id\" = ?1 AND \"repository_id\" = ?2",
            select_list(Table::TaskChangeSurface, None)
        ),
        params: vec![id_sql(task_id), id_sql(repository_id)],
    }
}

/// Decodes the single `COUNT(*)` column of a count query.
pub fn count(values: &[Raw]) -> Result<i64, WgError> {
    Cols::new(&["count"], values).int("count")
}

/// Decodes the single `sha` column of a [`commit_shas`] row.
pub fn commit_sha(values: &[Raw]) -> Result<String, WgError> {
    Cols::new(&["sha"], values).text("sha")
}

/// A `commit_task_links` row joined to its task: the link and the task's key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTaskLinkWithKey {
    pub link: CommitTaskLink,
    pub task_key: String,
}

impl CommitTaskLinkWithKey {
    /// Decodes a [`links_with_task_keys`] row: the link's columns, then the key.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        let (link_values, rest) = split_after(values, Table::CommitTaskLink)?;
        Ok(CommitTaskLinkWithKey {
            link: CommitTaskLink::from_raw(link_values)?,
            task_key: Cols::new(&["key"], rest).text("key")?,
        })
    }
}

/// A `commit_task_links` row joined to its commit and the commit's repository name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCommitLink {
    pub link: CommitTaskLink,
    pub commit: Commit,
    pub repository_name: String,
}

impl TaskCommitLink {
    /// Decodes a [`task_commit_links`] row: the link's columns, the commit's, then the name.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        let (link_values, rest) = split_after(values, Table::CommitTaskLink)?;
        let (commit_values, rest) = split_after(rest, Table::Commit)?;
        Ok(TaskCommitLink {
            link: CommitTaskLink::from_raw(link_values)?,
            commit: Commit::from_raw(commit_values)?,
            repository_name: Cols::new(&["name"], rest).text("name")?,
        })
    }
}

/// A [`path_totals`] row: one path of the commits' file changes, summed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathTotal {
    pub path: String,
    pub changes: i64,
    pub insertions: i64,
    pub deletions: i64,
}

impl PathTotal {
    /// Decodes a [`path_totals`] row.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        let cols = Cols::new(&["path", "changes", "insertions", "deletions"], values);
        Ok(PathTotal {
            path: cols.text("path")?,
            changes: cols.int("changes")?,
            insertions: cols.int("insertions")?,
            deletions: cols.int("deletions")?,
        })
    }
}

/// A [`commit_reachability`] row: a commit's id, SHA and reachability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReachability {
    pub id: Id,
    pub sha: String,
    pub reachable: bool,
}

impl CommitReachability {
    /// Decodes a [`commit_reachability`] row.
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        let cols = Cols::new(&["id", "sha", "reachable"], values);
        Ok(CommitReachability {
            id: cols.id("id")?,
            sha: cols.text("sha")?,
            reachable: cols.flag("reachable")?,
        })
    }
}

/// Splits a joined row after the columns of `table`, failing when the row is too short.
fn split_after(values: &[Raw], table: Table) -> Result<(&[Raw], &[Raw]), WgError> {
    let width = columns(table).len();
    if values.len() < width {
        return Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!(
                "joined row has {} values, expected at least {width} for {}",
                values.len(),
                table.name()
            ),
        ));
    }
    Ok(values.split_at(width))
}
