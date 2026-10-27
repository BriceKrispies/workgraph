//! The output shapes of the classification and embedding writes, and of similar-task search: the
//! replies of `request`, `reclassify`, `gold`, `evaluate`, `search --reindex` and `search
//! --similar-to`, and the results the embedding and evaluation jobs return.
//!
//! Key order follows the Python dictionaries and pydantic models, so `--json` output matches.

use serde_json::{Map, Value as Json};
use wg_json::{PyStr, Value};
use wg_model::entity::{Classification, ClassificationEvaluation};
use wg_types::Id;

use crate::spec::similar_sql::ImplementationCommit;
use wg_wire::convention::json_value;

fn text(value: &str) -> Value {
    Value::Str(PyStr::from(value))
}

fn optional_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, text)
}

fn dict(entries: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        entries
            .into_iter()
            .map(|(key, value)| (PyStr::from(key), value))
            .collect(),
    )
}

/// `request`'s reply: the queued job, the task and its key, and the job's status.
#[must_use]
pub fn request_out(job_id: Id, task_id: Id, task_key: &str, status: &str) -> Value {
    dict(vec![
        ("job_id", text(&job_id.to_string())),
        ("task_id", text(&task_id.to_string())),
        ("task_key", text(task_key)),
        ("status", text(status)),
    ])
}

/// `reclassify`'s reply: the batch, how many jobs it queued, their ids and the matching total.
#[must_use]
pub fn reclassify_out(batch_id: Id, job_ids: &[Id], total_matching: usize) -> Value {
    dict(vec![
        ("batch_id", text(&batch_id.to_string())),
        (
            "tasks",
            Value::Int(i128::try_from(job_ids.len()).unwrap_or(i128::MAX)),
        ),
        (
            "job_ids",
            Value::List(job_ids.iter().map(|id| text(&id.to_string())).collect()),
        ),
        (
            "total_matching",
            Value::Int(i128::try_from(total_matching).unwrap_or(i128::MAX)),
        ),
    ])
}

/// `gold --category`'s reply: the label's id, title and category.
#[must_use]
pub fn gold_added_out(id: Id, title: &str, category: &str) -> Value {
    dict(vec![
        ("id", text(&id.to_string())),
        ("title", text(title)),
        ("category", text(category)),
    ])
}

/// The result a `classify_task` job returns (`classification_id`, `status` and `category`).
#[must_use]
pub fn classify_result(row: &Classification) -> Map<String, Json> {
    let mut result = Map::new();
    result.insert(
        "classification_id".to_owned(),
        Json::String(row.id.to_string()),
    );
    result.insert("status".to_owned(), Json::String(row.status.clone()));
    result.insert(
        "category".to_owned(),
        row.category.clone().map_or(Json::Null, Json::String),
    );
    result
}

/// `evaluate`'s reply: the stored evaluation, its metrics and the per-label results.
#[must_use]
pub fn evaluation_out(
    row: &ClassificationEvaluation,
    metrics: &Json,
    results: &[Map<String, Json>],
) -> Value {
    let metric = |key: &str| metrics.get(key).map_or(Value::Null, json_value);
    dict(vec![
        ("id", text(&row.id.to_string())),
        ("provider", text(&row.provider)),
        ("model", text(&row.model)),
        ("prompt_version", text(&row.prompt_version)),
        ("taxonomy_version", text(&row.taxonomy_version)),
        ("sample_size", Value::Int(i128::from(row.sample_size))),
        ("failures", Value::Int(i128::from(row.failures))),
        ("accuracy", metric("accuracy")),
        ("macro_f1", metric("macro_f1")),
        ("per_class", metric("per_class")),
        ("confusion", metric("confusion")),
        (
            "results",
            Value::List(
                results
                    .iter()
                    .map(|entry| json_value(&Json::Object(entry.clone())))
                    .collect(),
            ),
        ),
    ])
}

/// The result an `evaluate_classifier` job returns: the evaluation id and its accuracy.
#[must_use]
pub fn evaluate_result(row: &ClassificationEvaluation) -> Map<String, Json> {
    let mut result = Map::new();
    result.insert("evaluation_id".to_owned(), Json::String(row.id.to_string()));
    result.insert(
        "accuracy".to_owned(),
        row.accuracy
            .map_or(Json::Null, |accuracy| Json::from(accuracy.0)),
    );
    result
}

/// The result an `embed_task` job returns (`embed_task`'s dictionary).
#[must_use]
pub fn embed_result(
    task_id: Id,
    status: &str,
    provider: &str,
    model: &str,
    dimensions: Option<usize>,
) -> Map<String, Json> {
    let mut result = Map::new();
    result.insert("task_id".to_owned(), Json::String(task_id.to_string()));
    result.insert("status".to_owned(), Json::String(status.to_owned()));
    result.insert("provider".to_owned(), Json::String(provider.to_owned()));
    result.insert("model".to_owned(), Json::String(model.to_owned()));
    if let Some(dimensions) = dimensions {
        result.insert("dimensions".to_owned(), Json::from(dimensions));
    }
    result
}

/// The result of `rebuild_embeddings` (`{"enqueued", "provider", "model"}`).
#[must_use]
pub fn rebuild_out(enqueued: usize, provider: Option<&str>, model: Option<&str>) -> Value {
    dict(vec![
        (
            "enqueued",
            Value::Int(i128::try_from(enqueued).unwrap_or(i128::MAX)),
        ),
        ("provider", optional_text(provider)),
        ("model", optional_text(model)),
    ])
}

/// One similar task with the evidence the listing shows.
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarEntry {
    pub task_id: Id,
    pub key: String,
    pub title: String,
    pub status: String,
    pub category: Option<String>,
    /// The score, unrounded (the order is by it).
    pub score: f64,
    /// The similarity, unrounded.
    pub similarity: f64,
    pub same_category: bool,
    pub overlap: f64,
    pub commits: Vec<ImplementationCommit>,
    pub attempts: i64,
}

/// `similar`'s reply: the task, the method, the embedding model (semantic only) and the entries.
#[must_use]
pub fn similar_out(
    task_id: Id,
    method: &str,
    embedding_model: Option<&str>,
    entries: &[SimilarEntry],
) -> Value {
    dict(vec![
        ("task_id", text(&task_id.to_string())),
        ("method", text(method)),
        ("embedding_model", optional_text(embedding_model)),
        (
            "similar",
            Value::List(entries.iter().map(similar_entry_out).collect()),
        ),
    ])
}

fn similar_entry_out(entry: &SimilarEntry) -> Value {
    dict(vec![
        ("task_id", text(&entry.task_id.to_string())),
        ("key", text(&entry.key)),
        ("title", text(&entry.title)),
        ("status", text(&entry.status)),
        ("category", optional_text(entry.category.as_deref())),
        ("score", Value::Float(round(entry.score, 4))),
        ("similarity", Value::Float(round(entry.similarity, 4))),
        ("same_category", Value::Bool(entry.same_category)),
        ("repository_overlap", Value::Float(round(entry.overlap, 3))),
        (
            "implementation",
            Value::List(
                entry
                    .commits
                    .iter()
                    .map(|commit| {
                        dict(vec![
                            ("sha", text(&commit.sha)),
                            ("subject", text(&commit.subject)),
                            ("repository_id", text(&commit.repository_id.to_string())),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("attempts", Value::Int(i128::from(entry.attempts))),
    ])
}

/// `round(x, digits)` as Python rounds it.
fn round(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// The similar entries the lookup keeps: the best `limit`, in the order given.
#[must_use]
pub fn best(entries: &[SimilarEntry], limit: usize) -> &[SimilarEntry] {
    &entries[..entries.len().min(limit)]
}
