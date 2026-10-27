//! The lexical part of `SearchService.search` as pure decisions: which candidates are hits, their
//! scores and ranks, and the `SearchResult` shape the `search` command prints.
//!
//! The candidates come from [`crate::spec::search_sql::lexical_candidates`] and the scope and task
//! reads of [`crate::service`]; this module only decides and shapes.

use wg_json::Value;
use wg_registry::resolve;
use wg_store::TaskWorld;
use wg_types::{Id, WgError};

use crate::spec::search_sql::SearchScope;
use wg_wire::convention::{optional_text, text};

/// The candidate limit `search` passes to `lexical` (`limit * 2`).
const LEXICAL_WIDEN: i64 = 2;

/// The task fields a hit shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskHead {
    pub key: String,
    pub title: String,
    pub status: String,
    pub category: Option<String>,
}

/// One bm25 candidate, in rank order, with its scope membership and its task (if it still exists).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub task_id: Id,
    /// bm25: lower is better.
    pub rank: f64,
    pub snippet: String,
    pub in_scope: bool,
    pub task: Option<TaskHead>,
}

/// A lexical `SearchHit`.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub task_id: Id,
    pub head: TaskHead,
    pub score: f64,
    pub lexical_rank: i64,
    pub snippet: String,
}

/// The note of an unavailable semantic leg (`SearchService.search`).
#[must_use]
pub fn unavailable_note(provider: &str, model: &str) -> String {
    format!(
        "semantic search unavailable: no embeddings stored for {provider}/{model}; run `workgraph search reindex`; lexical results only"
    )
}

/// `lexical(..., limit=limit * 2)`: the number of hits `search` asks the lexical leg for.
#[must_use]
pub fn lexical_widened(limit: i64) -> i64 {
    limit * LEXICAL_WIDEN
}

/// `round(x, 4)`: the value rounded to four decimals, as Python rounds it.
#[must_use]
pub fn round4(value: f64) -> f64 {
    format!("{value:.4}").parse().unwrap_or(value)
}

/// The lexical hits of `candidates`, best first: candidates in scope whose task exists, cut to
/// `limit` (`SearchService.lexical`).
#[must_use]
pub fn lexical_hits(candidates: &[Candidate], limit: i64) -> Vec<Hit> {
    let limit = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    candidates
        .iter()
        .filter(|candidate| candidate.in_scope)
        .filter_map(|candidate| {
            candidate
                .task
                .as_ref()
                .map(|head| (candidate, head.clone()))
        })
        .take(limit)
        .enumerate()
        .map(|(at, (candidate, head))| Hit {
            task_id: candidate.task_id,
            head,
            score: round4(-candidate.rank),
            lexical_rank: i64::try_from(at + 1).unwrap_or(i64::MAX),
            snippet: candidate.snippet.clone(),
        })
        .collect()
}

/// A `SearchHit` as the service returns it (`model_dump`), with the semantic fields unset.
#[must_use]
pub fn hit_out(hit: &Hit) -> Value {
    Value::Model {
        name: "SearchHit".to_owned(),
        fields: vec![
            ("task_id".to_owned(), text(&hit.task_id.to_string())),
            ("key".to_owned(), text(&hit.head.key)),
            ("title".to_owned(), text(&hit.head.title)),
            ("status".to_owned(), text(&hit.head.status)),
            (
                "category".to_owned(),
                optional_text(hit.head.category.as_deref()),
            ),
            ("score".to_owned(), Value::Float(hit.score)),
            (
                "lexical_rank".to_owned(),
                Value::Int(i128::from(hit.lexical_rank)),
            ),
            ("semantic_rank".to_owned(), Value::Null),
            ("similarity".to_owned(), Value::Null),
            ("snippet".to_owned(), text(&hit.snippet)),
        ],
    }
}

/// The `SearchResult` `SearchService.search` returns for the modes served here. Those never have a
/// semantic leg, so `semantic_available` is false and `embedding_model` is null.
#[must_use]
pub fn outcome(
    query: &str,
    mode: &str,
    candidates: &[Candidate],
    limit: i64,
    unavailable: Option<&str>,
) -> Value {
    if let (true, Some(note)) = (mode == "semantic", unavailable) {
        return result_out(query, mode, &[], Some(note));
    }
    let cap = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    let hits: Vec<Hit> = lexical_hits(candidates, lexical_widened(limit))
        .into_iter()
        .take(cap)
        .collect();
    result_out(query, "lexical", &hits, unavailable)
}

/// The `SearchResult` shape: `hits` already cut to the caller's limit.
#[must_use]
fn result_out(query: &str, mode: &str, hits: &[Hit], note: Option<&str>) -> Value {
    Value::Model {
        name: "SearchResult".to_owned(),
        fields: vec![
            ("query".to_owned(), text(query)),
            ("mode".to_owned(), text(mode)),
            ("semantic_available".to_owned(), Value::Bool(false)),
            ("embedding_model".to_owned(), Value::Null),
            (
                "hits".to_owned(),
                Value::List(hits.iter().map(hit_out).collect()),
            ),
            ("note".to_owned(), optional_text(note)),
        ],
    }
}

/// The filters `search` takes, as the command gives them (empty strings mean no filter).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchFilters {
    pub project: Option<String>,
    pub repository: Option<String>,
    pub category: Option<String>,
    pub statuses: Vec<String>,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub changed_path: Option<String>,
}

/// The scope of a search, with project and repository resolved to ids.
pub fn scope_of(
    world: &TaskWorld,
    ws: Id,
    filters: &SearchFilters,
) -> Result<SearchScope, WgError> {
    fn present(text: &Option<String>) -> Option<&str> {
        text.as_deref().filter(|value| !value.is_empty())
    }
    let project_id = match present(&filters.project) {
        Some(reference) => Some(resolve::project(&world.tasks.registry, ws, reference)?.id),
        None => None,
    };
    let repository_id = match present(&filters.repository) {
        Some(reference) => {
            Some(resolve::repository(&world.tasks.registry, ws, reference, true)?.id)
        }
        None => None,
    };
    Ok(SearchScope {
        include_archived: false,
        project_id,
        repository_id,
        category: present(&filters.category).map(str::to_owned),
        statuses: filters.statuses.clone(),
        created_after: None,
        created_before: None,
        agent: present(&filters.agent).map(str::to_owned),
        model: present(&filters.model).map(str::to_owned),
        changed_path: present(&filters.changed_path).map(str::to_owned),
    })
}

pub fn head_of(world: &TaskWorld, id: Id) -> Option<TaskHead> {
    world.tasks.task(id).map(|task| TaskHead {
        key: task.key.to_string(),
        title: task.title.clone(),
        status: task.status.as_str().to_owned(),
        category: task.category.clone(),
    })
}
