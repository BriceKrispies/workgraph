//! SQL text, parameters and typed rows for search (SH-11): the FTS5 MATCH expression built from a
//! user's text (`search/indexer.py`'s `to_fts_query`), the bm25 candidate query of the lexical
//! search (`search/service.py`'s `lexical`), the scope query of the search filters (`_scope`), and
//! the embedding reads (`_vectors` and the lookup `embed_task` makes).
//!
//! The bm25 statement reproduces the text `search/service.py` issues, and the MATCH expression is
//! the same string `to_fts_query` returns, so the ranking is the one Python produces.

use std::fmt::Write as _;

use rpds::VectorSync;
use wg_model::Table;
use wg_types::{Id, Timestamp, WgError};

use wg_storecodec::codec::{Cols, Raw, id_sql, text_sql, ts_sql};
use wg_storecodec::sql::{Statement, select_list};

/// `search/service.py` fetches `limit * LEXICAL_OVERFETCH` candidates, because rows outside the
/// filters' scope are dropped after the query.
pub const LEXICAL_OVERFETCH: i64 = 5;

/// The bm25 weights per `task_fts` column, in the table's column order (`task_id`, `workspace_id`,
/// `key`, `title`, `description`, `comments`, `tags`, `summary`). The two id columns weigh 0.
const BM25_WEIGHTS: &str = "0, 0, 5.0, 10.0, 3.0, 1.0, 2.0, 2.0";

/// Python's `\w`, as `[\w]+` matches it: letters and digits of any script, and underscore.
fn is_word_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// `to_fts_query`: a safe FTS5 MATCH expression for free text (no operator injection, since every
/// token is quoted), or `None` when the text has no word characters.
///
/// Each token is double-quoted. With `prefix`, the last token gets a `*` wildcard for
/// search-as-you-type. Tokens are AND-ed unless `match_any`, which OR-s the first 40 distinct
/// tokens (similarity lookups: a shared term counts, and bm25 ranks documents sharing more terms
/// higher).
pub fn fts_match(user_query: &str, prefix: bool, match_any: bool) -> Option<String> {
    let mut quoted: Vec<String> = user_query
        .split(|c: char| !is_word_char(c))
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{token}\""))
        .collect();
    if quoted.is_empty() {
        return None;
    }
    if let Some(last) = quoted.last_mut().filter(|_| prefix) {
        last.push('*');
    }
    if match_any {
        let mut distinct: Vec<&str> = Vec::new();
        for token in quoted.iter().take(40) {
            if !distinct.contains(&token.as_str()) {
                distinct.push(token);
            }
        }
        return Some(distinct.join(" OR "));
    }
    Some(quoted.join(" "))
}

/// The bm25 candidates of `match_expr` in `workspace_id`, best first, at most `limit *
/// LEXICAL_OVERFETCH` of them. Each row is `task_id`, `rank` (bm25: lower is better) and the
/// highlighted snippet.
pub fn lexical_candidates(match_expr: &str, workspace_id: Id, limit: i64) -> Statement {
    Statement {
        sql: format!(
            "SELECT task_id, bm25(task_fts, {BM25_WEIGHTS}) AS rank, \
             snippet(task_fts, -1, '[', ']', ' ... ', 12) AS snip \
             FROM task_fts WHERE task_fts MATCH ?1 AND workspace_id = ?2 \
             ORDER BY rank LIMIT ?3"
        ),
        params: vec![
            text_sql(match_expr),
            id_sql(workspace_id),
            Raw::Int(limit * LEXICAL_OVERFETCH),
        ],
    }
}

/// One lexical candidate, as [`lexical_candidates`] returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct FtsCandidate {
    pub task_id: Id,
    /// The bm25 value; the service reports `-rank` as the score.
    pub rank: f64,
    pub snippet: String,
}

impl FtsCandidate {
    /// Decodes a row of [`lexical_candidates`].
    pub fn from_raw(values: &[Raw]) -> Result<Self, WgError> {
        let cols = Cols::new(&["task_id", "rank", "snip"], values);
        Ok(FtsCandidate {
            task_id: cols.id("task_id")?,
            rank: cols.real("rank")?.0,
            snippet: cols.text("snip")?,
        })
    }
}

/// The filters of a search (`SearchFilters`), with the names the service resolves already turned
/// into ids. An empty `statuses` list means no status filter, as in the service.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchScope {
    /// Archived tasks are left out unless this is set.
    pub include_archived: bool,
    /// The project the service resolved from `project`, if one was asked for.
    pub project_id: Option<Id>,
    /// The repository the service resolved from `repository` (archived ones included), if asked.
    pub repository_id: Option<Id>,
    pub category: Option<String>,
    pub statuses: Vec<String>,
    /// `created_at >= created_after`.
    pub created_after: Option<Timestamp>,
    /// `created_at < created_before`.
    pub created_before: Option<Timestamp>,
    /// The agent adapter (`provider`) of some attempt on the task.
    pub agent: Option<String>,
    /// The model of some attempt on the task.
    pub model: Option<String>,
    /// A substring of a path changed by a commit linked to the task.
    pub changed_path: Option<String>,
}

/// Binds `value` and returns its placeholder, `?N`.
fn bind(params: &mut Vec<Raw>, value: Raw) -> String {
    params.push(value);
    format!("?{}", params.len())
}

/// The ids of the tasks of `workspace_id` that pass `scope` (`SearchService._scope`). Unordered:
/// the caller uses the result as a set.
pub fn search_scope(workspace_id: Id, scope: &SearchScope) -> Statement {
    let mut params = vec![id_sql(workspace_id)];
    let mut sql = String::from("SELECT \"id\" FROM \"tasks\" WHERE \"workspace_id\" = ?1");
    if !scope.include_archived {
        sql.push_str(" AND \"archived_at\" IS NULL");
    }
    if let Some(project_id) = scope.project_id {
        let m = bind(&mut params, id_sql(project_id));
        let _ = write!(
            sql,
            " AND \"id\" IN (SELECT \"task_id\" FROM \"{}\" WHERE \"project_id\" = {m})",
            Table::TaskProject.name()
        );
    }
    if let Some(repository_id) = scope.repository_id {
        let m = bind(&mut params, id_sql(repository_id));
        let _ = write!(
            sql,
            " AND \"id\" IN (SELECT \"task_id\" FROM \"{}\" WHERE \"repository_id\" = {m})",
            Table::TaskRepository.name()
        );
    }
    if let Some(category) = &scope.category {
        let m = bind(&mut params, text_sql(category));
        let _ = write!(sql, " AND \"category\" = {m}");
    }
    if !scope.statuses.is_empty() {
        let marks: Vec<String> = scope
            .statuses
            .iter()
            .map(|status| bind(&mut params, text_sql(status)))
            .collect();
        let _ = write!(sql, " AND \"status\" IN ({})", marks.join(", "));
    }
    if let Some(at) = scope.created_after {
        let m = bind(&mut params, ts_sql(at));
        let _ = write!(sql, " AND \"created_at\" >= {m}");
    }
    if let Some(at) = scope.created_before {
        let m = bind(&mut params, ts_sql(at));
        let _ = write!(sql, " AND \"created_at\" < {m}");
    }
    if let Some(agent) = &scope.agent {
        let m = bind(&mut params, text_sql(agent));
        let _ = write!(
            sql,
            " AND \"id\" IN (SELECT \"task_id\" FROM \"{}\" WHERE \"provider\" = {m})",
            Table::AgentAttempt.name()
        );
    }
    if let Some(model) = &scope.model {
        let m = bind(&mut params, text_sql(model));
        let _ = write!(
            sql,
            " AND \"id\" IN (SELECT \"task_id\" FROM \"{}\" WHERE \"model\" = {m})",
            Table::AgentAttempt.name()
        );
    }
    if let Some(path) = &scope.changed_path {
        let m = bind(&mut params, text_sql(path));
        let _ = write!(
            sql,
            " AND \"id\" IN (SELECT l.\"task_id\" FROM \"{links}\" AS l \
             JOIN \"{changes}\" AS f ON f.\"commit_id\" = l.\"commit_id\" \
             WHERE f.\"path\" LIKE '%' || {m} || '%')",
            links = Table::CommitTaskLink.name(),
            changes = Table::CommitFileChange.name(),
        );
    }
    Statement { sql, params }
}

/// Decodes the task id from a row of [`search_scope`].
pub fn scoped_task_id(values: &[Raw]) -> Result<Id, WgError> {
    Cols::new(&["id"], values).id("id")
}

/// The embedding `task_id` has under `provider`/`model`, if any. The table's unique key makes it
/// at most one row (`embed_task`'s lookup).
pub fn task_embedding(task_id: Id, provider: &str, model: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"task_embeddings\" WHERE \"task_id\" = ?1 AND \"provider\" = ?2 \
             AND \"model\" = ?3 LIMIT 1",
            select_list(Table::TaskEmbedding, None)
        ),
        params: vec![id_sql(task_id), text_sql(provider), text_sql(model)],
    }
}

/// Every stored vector of `workspace_id` under `provider`/`model` (the scan behind `semantic`
/// and `similar`).
pub fn embedding_vectors(workspace_id: Id, provider: &str, model: &str) -> Statement {
    Statement {
        sql: "SELECT e.\"task_id\", e.\"vector\" FROM \"task_embeddings\" AS e \
              JOIN \"tasks\" AS t ON t.\"id\" = e.\"task_id\" \
              WHERE t.\"workspace_id\" = ?1 AND e.\"provider\" = ?2 AND e.\"model\" = ?3"
            .to_owned(),
        params: vec![id_sql(workspace_id), text_sql(provider), text_sql(model)],
    }
}

/// One stored vector, as [`embedding_vectors`] returns it: the packed bytes, unpacked by the
/// caller.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskVector {
    pub task_id: Id,
    pub vector: VectorSync<u8>,
}

wg_storecodec::row_projection! {
    TaskVector {
        task_id: id,
        vector: blob,
    }
}
