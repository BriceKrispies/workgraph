//! `SearchHit` and `SearchResult`: the answer of `GET /search` (MCP `task_search`).
//!
//! Both are Pydantic models, so they print as models; neither carries a timestamp.

use wg_json::Value;

use crate::dto::Dto;
use crate::wire_dto;

wire_dto! {
    /// One ranked hit. `score` is the fused score; the ranks and similarity are set only by the
    /// rankings that found the task.
    pub struct Hit<'a>("SearchHit") {
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
}

wire_dto! {
    /// A search result. `hits` are the [`hit_from_parts`] values, best first.
    pub struct SearchResult<'a>("SearchResult")
    value from_parts(query: &'a str, mode: &'a str, semantic_available: bool, embedding_model: Option<&'a str>, hits: Vec<Value>, note: Option<&'a str>) {
        query: &'a str = query,
        mode: &'a str = mode,
        semantic_available: bool = semantic_available,
        embedding_model: Option<&'a str> = embedding_model,
        hits: Vec<Value> = hits,
        note: Option<&'a str> = note,
    }
}

/// A hit as a model value.
pub fn hit_from_parts(hit: &Hit<'_>) -> Value {
    hit.value()
}
