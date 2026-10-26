//! The output shapes of the `classify` reads (`ClassificationService.get`, `gold`, `evaluations`,
//! `disagreements`): pure functions from the stored rows to the values the commands print.
//!
//! Key order follows the Python dicts and pydantic models, so `--json` output matches.

use std::collections::BTreeSet;

use wg_json::{PyStr, Value};
use wg_model::entity::{Classification, ClassificationEvaluation, ClassificationGoldLabel};
use wg_types::Id;

/// The note `TaskClassifications` carries.
pub const NOTE: &str = "Model classifications are inferences, not objective facts.";

fn text(value: &str) -> Value {
    Value::Str(PyStr::from(value))
}

fn optional_text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, text)
}

fn texts(values: impl Iterator<Item = impl AsRef<str>>) -> Value {
    Value::List(values.map(|v| text(v.as_ref())).collect())
}

fn dict(entries: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        entries
            .into_iter()
            .map(|(key, value)| (PyStr::from(key), value))
            .collect(),
    )
}

/// A `serde_json` value as a Python-shaped value (object key order kept).
#[must_use]
pub fn json_value(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(flag) => Value::Bool(*flag),
        serde_json::Value::Number(number) => number.as_i64().map_or_else(
            || Value::Float(number.as_f64().unwrap_or(f64::NAN)),
            |int| Value::Int(i128::from(int)),
        ),
        serde_json::Value::String(text_value) => text(text_value),
        serde_json::Value::Array(items) => Value::List(items.iter().map(json_value).collect()),
        serde_json::Value::Object(map) => Value::Dict(
            map.iter()
                .map(|(key, value)| (PyStr::from(key.as_str()), json_value(value)))
                .collect(),
        ),
    }
}

fn optional_float(value: Option<f64>) -> Value {
    value.map_or(Value::Null, Value::Float)
}

/// A `ClassificationOut`.
#[must_use]
pub fn classification_out(row: &Classification) -> Value {
    Value::Model {
        name: "ClassificationOut".to_owned(),
        fields: vec![
            ("id".to_owned(), text(&row.id.to_string())),
            ("task_id".to_owned(), text(&row.task_id.to_string())),
            ("source".to_owned(), text(&row.source)),
            ("status".to_owned(), text(&row.status)),
            (
                "provider".to_owned(),
                optional_text(row.provider.as_deref()),
            ),
            ("model".to_owned(), optional_text(row.model.as_deref())),
            (
                "prompt_version".to_owned(),
                optional_text(row.prompt_version.as_deref()),
            ),
            ("taxonomy_version".to_owned(), text(&row.taxonomy_version)),
            ("input_hash".to_owned(), text(&row.input_hash)),
            (
                "category".to_owned(),
                optional_text(row.category.as_deref()),
            ),
            ("subtype".to_owned(), optional_text(row.subtype.as_deref())),
            ("domain".to_owned(), optional_text(row.domain.as_deref())),
            (
                "affected_systems".to_owned(),
                texts(row.affected_systems.iter()),
            ),
            (
                "complexity".to_owned(),
                optional_text(row.complexity.as_deref()),
            ),
            ("tags".to_owned(), texts(row.tags.iter())),
            (
                "confidence".to_owned(),
                optional_float(row.confidence.map(|real| real.0)),
            ),
            (
                "rationale".to_owned(),
                optional_text(row.rationale.as_deref()),
            ),
            ("validation_status".to_owned(), text(&row.validation_status)),
            ("error".to_owned(), optional_text(row.error.as_deref())),
            (
                "duration_ms".to_owned(),
                row.duration_ms
                    .map_or(Value::Null, |ms| Value::Int(i128::from(ms))),
            ),
            ("is_current".to_owned(), Value::Bool(row.is_current)),
            (
                "confirmed_by".to_owned(),
                optional_text(row.confirmed_by.as_deref()),
            ),
            (
                "batch_id".to_owned(),
                row.batch_id.map_or(Value::Null, |id| text(&id.to_string())),
            ),
            ("created_by".to_owned(), text(&row.created_by)),
            ("created_at".to_owned(), Value::DateTime(row.created_at)),
        ],
    }
}

/// A `TaskClassifications`: the current classification, the history (newest first, as given) and
/// whether the succeeded classifications name more than one category.
#[must_use]
pub fn show_out(
    task_id: Id,
    task_key: &str,
    current_id: Option<Id>,
    history: &[Classification],
) -> Value {
    let current = current_id.and_then(|id| history.iter().find(|row| row.id == id));
    let categories: BTreeSet<&str> = history
        .iter()
        .filter(|row| row.status == "succeeded")
        .filter_map(|row| row.category.as_deref())
        .collect();
    Value::Model {
        name: "TaskClassifications".to_owned(),
        fields: vec![
            ("task_id".to_owned(), text(&task_id.to_string())),
            ("task_key".to_owned(), text(task_key)),
            (
                "current".to_owned(),
                current.map_or(Value::Null, classification_out),
            ),
            (
                "history".to_owned(),
                Value::List(history.iter().map(classification_out).collect()),
            ),
            ("disagreement".to_owned(), Value::Bool(categories.len() > 1)),
            ("note".to_owned(), text(NOTE)),
        ],
    }
}

/// The rows of `gold`: `{id, task_id, title, category, labeled_by}`.
#[must_use]
pub fn gold_out(rows: &[ClassificationGoldLabel]) -> Value {
    Value::List(
        rows.iter()
            .map(|row| {
                dict(vec![
                    ("id", text(&row.id.to_string())),
                    (
                        "task_id",
                        row.task_id.map_or(Value::Null, |id| text(&id.to_string())),
                    ),
                    ("title", text(&row.title)),
                    ("category", text(&row.category)),
                    ("labeled_by", text(&row.labeled_by)),
                ])
            })
            .collect(),
    )
}

/// The rows of `evaluations`, newest first as given.
#[must_use]
pub fn evaluations_out(rows: &[ClassificationEvaluation]) -> Value {
    Value::List(
        rows.iter()
            .map(|row| {
                dict(vec![
                    ("id", text(&row.id.to_string())),
                    ("provider", text(&row.provider)),
                    ("model", text(&row.model)),
                    ("prompt_version", text(&row.prompt_version)),
                    ("taxonomy_version", text(&row.taxonomy_version)),
                    ("sample_size", Value::Int(i128::from(row.sample_size))),
                    ("failures", Value::Int(i128::from(row.failures))),
                    ("accuracy", optional_float(row.accuracy.map(|real| real.0))),
                    ("macro_f1", optional_float(row.macro_f1.map(|real| real.0))),
                    (
                        "per_class",
                        json_value(&serde_json::Value::Object(row.per_class.clone())),
                    ),
                    (
                        "confusion",
                        json_value(&serde_json::Value::Object(row.confusion.clone())),
                    ),
                    ("created_at", text(&row.created_at.format_isoformat())),
                ])
            })
            .collect(),
    )
}

/// The succeeded classification of a task in `disagreements`, as the command shows it.
#[must_use]
fn disagreement_item(row: &Classification) -> Value {
    dict(vec![
        ("id", text(&row.id.to_string())),
        ("category", optional_text(row.category.as_deref())),
        ("source", text(&row.source)),
        ("provider", optional_text(row.provider.as_deref())),
        ("model", optional_text(row.model.as_deref())),
        ("is_current", Value::Bool(row.is_current)),
        (
            "confidence",
            optional_float(row.confidence.map(|real| real.0)),
        ),
    ])
}

/// The tasks whose succeeded classifications name more than one category, in first-appearance
/// order. A missing category counts as a category of its own (`Counter` over `r.category`).
#[must_use]
pub fn disagreements_out(rows: &[(Classification, String)]) -> Value {
    let mut order: Vec<Id> = Vec::new();
    for (row, _) in rows {
        if !order.contains(&row.task_id) {
            order.push(row.task_id);
        }
    }
    Value::List(
        order
            .into_iter()
            .filter_map(|task_id| {
                let items: Vec<&(Classification, String)> = rows
                    .iter()
                    .filter(|(row, _)| row.task_id == task_id)
                    .collect();
                let categories: BTreeSet<Option<&str>> = items
                    .iter()
                    .map(|(row, _)| row.category.as_deref())
                    .collect();
                (categories.len() > 1).then(|| {
                    dict(vec![
                        ("task_id", text(&task_id.to_string())),
                        ("task_key", text(&items[0].1)),
                        (
                            "classifications",
                            Value::List(
                                items
                                    .iter()
                                    .map(|(row, _)| disagreement_item(row))
                                    .collect(),
                            ),
                        ),
                    ])
                })
            })
            .collect(),
    )
}
