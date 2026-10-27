//! The semantic and hybrid legs of `SearchService.search` as pure decisions: the reciprocal rank
//! fusion of the lexical and semantic rankings, and the `SearchHit` and `SearchResult` shapes the
//! command prints.
//!
//! The ranking inputs (bm25 candidates, cosine scores of the stored vectors) come from the shell;
//! this module orders and shapes them.

use wg_intel::RRF_K;
use wg_json::Value;
use wg_types::Id;

use crate::spec::search_out::{Hit, TaskHead, round4};
use wg_wire::convention::text;

/// One semantic candidate: the task, its head and its cosine similarity to the query.
#[derive(Debug, Clone, PartialEq)]
pub struct Semantic {
    pub task_id: Id,
    pub head: TaskHead,
    /// The cosine similarity, unrounded (the score and the sort use this).
    pub similarity: f64,
}

/// A `SearchHit` as the service builds it, with the ranks of each leg that found it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutHit {
    pub task_id: Id,
    pub head: TaskHead,
    pub score: f64,
    pub lexical_rank: Option<i64>,
    pub semantic_rank: Option<i64>,
    pub similarity: Option<f64>,
    pub snippet: Option<String>,
}

/// The semantic hits: the candidates in the order given (best first), with their 1-based rank and
/// the similarity rounded to four decimals, cut to `limit`.
#[must_use]
pub fn semantic_hits(candidates: &[Semantic], limit: i64) -> Vec<OutHit> {
    let cap = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    candidates
        .iter()
        .take(cap)
        .enumerate()
        .map(|(at, candidate)| OutHit {
            task_id: candidate.task_id,
            head: candidate.head.clone(),
            score: round4(candidate.similarity),
            lexical_rank: None,
            semantic_rank: i64::try_from(at + 1).ok(),
            similarity: Some(round4(candidate.similarity)),
            snippet: None,
        })
        .collect()
}

/// The lexical hits as output hits.
#[must_use]
pub fn lexical_out_hits(hits: &[Hit]) -> Vec<OutHit> {
    hits.iter()
        .map(|hit| OutHit {
            task_id: hit.task_id,
            head: hit.head.clone(),
            score: hit.score,
            lexical_rank: Some(hit.lexical_rank),
            semantic_rank: None,
            similarity: None,
            snippet: Some(hit.snippet.clone()),
        })
        .collect()
}

/// Reciprocal rank fusion (`mode == "hybrid"`): the lexical hits and the semantic hits scored by
/// `1 / (RRF_K + rank)` each, ordered best first (a stable sort by the negated score), with the
/// ranks and the similarity of each leg that found the task. Scores are rounded to six decimals.
#[must_use]
pub fn fuse(lexical: &[OutHit], semantic: &[OutHit], limit: i64) -> Vec<OutHit> {
    let k = f64::from(RRF_K);
    // The tasks in first-appearance order: the lexical hits, then the semantic ones.
    let order: Vec<Id> =
        lexical
            .iter()
            .chain(semantic.iter())
            .fold(Vec::new(), |seen: Vec<Id>, hit| {
                if seen.contains(&hit.task_id) {
                    seen
                } else {
                    seen.into_iter().chain([hit.task_id]).collect()
                }
            });
    let fused: Vec<(f64, OutHit)> = order
        .iter()
        .filter_map(|id| {
            let lex = lexical.iter().find(|hit| hit.task_id == *id);
            let sem = semantic.iter().find(|hit| hit.task_id == *id);
            let base = lex.or(sem)?;
            let score = lex
                .and_then(|hit| hit.lexical_rank)
                .map_or(0.0, |rank| 1.0 / (k + rank as f64))
                + sem
                    .and_then(|hit| hit.semantic_rank)
                    .map_or(0.0, |rank| 1.0 / (k + rank as f64));
            Some((
                score,
                OutHit {
                    task_id: *id,
                    head: base.head.clone(),
                    score,
                    lexical_rank: lex.and_then(|hit| hit.lexical_rank),
                    semantic_rank: sem.and_then(|hit| hit.semantic_rank),
                    similarity: sem.and_then(|hit| hit.similarity),
                    snippet: lex.and_then(|hit| hit.snippet.clone()),
                },
            ))
        })
        .collect();
    let mut ranked = fused;
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let cap = usize::try_from(limit.max(0)).unwrap_or(usize::MAX);
    ranked
        .into_iter()
        .take(cap)
        .map(|(_, hit)| OutHit {
            score: round6(hit.score),
            ..hit
        })
        .collect()
}

/// `round(x, 6)` as Python rounds it.
fn round6(value: f64) -> f64 {
    format!("{value:.6}").parse().unwrap_or(value)
}

/// One `SearchHit` as the service returns it (`model_dump`).
#[must_use]
pub fn out_hit_value(hit: &OutHit) -> Value {
    Value::Model {
        name: "SearchHit".to_owned(),
        fields: vec![
            ("task_id".to_owned(), text(&hit.task_id.to_string())),
            ("key".to_owned(), text(&hit.head.key)),
            ("title".to_owned(), text(&hit.head.title)),
            ("status".to_owned(), text(&hit.head.status)),
            (
                "category".to_owned(),
                hit.head.category.as_deref().map_or(Value::Null, text),
            ),
            ("score".to_owned(), Value::Float(hit.score)),
            (
                "lexical_rank".to_owned(),
                hit.lexical_rank
                    .map_or(Value::Null, |rank| Value::Int(i128::from(rank))),
            ),
            (
                "semantic_rank".to_owned(),
                hit.semantic_rank
                    .map_or(Value::Null, |rank| Value::Int(i128::from(rank))),
            ),
            (
                "similarity".to_owned(),
                hit.similarity.map_or(Value::Null, Value::Float),
            ),
            (
                "snippet".to_owned(),
                hit.snippet.as_deref().map_or(Value::Null, text),
            ),
        ],
    }
}

/// The `SearchResult` of a search: its mode, whether the semantic leg answered, the embedding
/// model it used, the hits and the note.
#[must_use]
pub fn result_value(
    query: &str,
    mode: &str,
    semantic_available: bool,
    embedding_model: Option<&str>,
    hits: &[OutHit],
    note: Option<&str>,
) -> Value {
    Value::Model {
        name: "SearchResult".to_owned(),
        fields: vec![
            ("query".to_owned(), text(query)),
            ("mode".to_owned(), text(mode)),
            (
                "semantic_available".to_owned(),
                Value::Bool(semantic_available),
            ),
            (
                "embedding_model".to_owned(),
                embedding_model.map_or(Value::Null, text),
            ),
            (
                "hits".to_owned(),
                Value::List(hits.iter().map(out_hit_value).collect()),
            ),
            ("note".to_owned(), note.map_or(Value::Null, text)),
        ],
    }
}
