//! `SearchHit` and `SearchResult`: the answer of `GET /search` (MCP `task_search`).
//!
//! Both are Pydantic models, so they print as models; neither carries a timestamp.

use wg_json::Value;

use crate::convention::{model, optional_float, optional_int, optional_text, text};

/// The name of the hit model.
pub const HIT_NAME: &str = "SearchHit";

/// The fields of a hit, in Python declaration order.
pub const HIT_FIELDS: [&str; 10] = [
    "task_id",
    "key",
    "title",
    "status",
    "category",
    "score",
    "lexical_rank",
    "semantic_rank",
    "similarity",
    "snippet",
];

/// The name of the result model.
pub const NAME: &str = "SearchResult";

/// The fields of a result, in Python declaration order.
pub const FIELDS: [&str; 6] = [
    "query",
    "mode",
    "semantic_available",
    "embedding_model",
    "hits",
    "note",
];

/// One ranked hit. `score` is the fused score; the ranks and similarity are set only by the
/// rankings that found the task.
#[derive(Debug, Clone)]
pub struct Hit<'a> {
    pub task_id: &'a str,
    pub key: &'a str,
    pub title: &'a str,
    pub status: &'a str,
    pub category: Option<&'a str>,
    pub score: f64,
    pub lexical_rank: Option<i64>,
    pub semantic_rank: Option<i64>,
    pub similarity: Option<f64>,
    pub snippet: Option<&'a str>,
}

/// A hit as a model value.
pub fn hit_from_parts(hit: &Hit<'_>) -> Value {
    model(
        HIT_NAME,
        vec![
            ("task_id", text(hit.task_id)),
            ("key", text(hit.key)),
            ("title", text(hit.title)),
            ("status", text(hit.status)),
            ("category", optional_text(hit.category)),
            ("score", Value::Float(hit.score)),
            ("lexical_rank", optional_int(hit.lexical_rank)),
            ("semantic_rank", optional_int(hit.semantic_rank)),
            ("similarity", optional_float(hit.similarity)),
            ("snippet", optional_text(hit.snippet)),
        ],
    )
}

/// A search result. `hits` are the [`hit_from_parts`] values, best first.
pub fn from_parts(
    query: &str,
    mode: &str,
    semantic_available: bool,
    embedding_model: Option<&str>,
    hits: Vec<Value>,
    note: Option<&str>,
) -> Value {
    model(
        NAME,
        vec![
            ("query", text(query)),
            ("mode", text(mode)),
            ("semantic_available", Value::Bool(semantic_available)),
            ("embedding_model", optional_text(embedding_model)),
            ("hits", Value::List(hits)),
            ("note", optional_text(note)),
        ],
    )
}
