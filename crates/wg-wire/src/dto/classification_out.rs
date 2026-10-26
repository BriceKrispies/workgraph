//! `ClassificationOut`: one classification of a task, as `classification_get` returns it inside
//! `TaskClassifications`.

use wg_json::Value;
use wg_model::entity::classification::Classification;

use crate::convention::{
    TimeFormat, id, model, optional_float, optional_id, optional_int, optional_text, text, texts,
    timestamp,
};

/// The OpenAPI component schema this DTO mirrors. It is not a component of the document, so the
/// name is the Python class name only.
pub const NAME: &str = "ClassificationOut";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 25] = [
    "id",
    "task_id",
    "source",
    "status",
    "provider",
    "model",
    "prompt_version",
    "taxonomy_version",
    "input_hash",
    "category",
    "subtype",
    "domain",
    "affected_systems",
    "complexity",
    "tags",
    "confidence",
    "rationale",
    "validation_status",
    "error",
    "duration_ms",
    "is_current",
    "confirmed_by",
    "batch_id",
    "created_by",
    "created_at",
];

/// A classification row. A Pydantic model, so `created_at` prints in the `Z` form.
pub fn from_model(row: &Classification) -> Value {
    model(
        NAME,
        vec![
            ("id", id(row.id)),
            ("task_id", id(row.task_id)),
            ("source", text(&row.source)),
            ("status", text(&row.status)),
            ("provider", optional_text(row.provider.as_deref())),
            ("model", optional_text(row.model.as_deref())),
            (
                "prompt_version",
                optional_text(row.prompt_version.as_deref()),
            ),
            ("taxonomy_version", text(&row.taxonomy_version)),
            ("input_hash", text(&row.input_hash)),
            ("category", optional_text(row.category.as_deref())),
            ("subtype", optional_text(row.subtype.as_deref())),
            ("domain", optional_text(row.domain.as_deref())),
            (
                "affected_systems",
                texts(row.affected_systems.iter().map(String::as_str)),
            ),
            ("complexity", optional_text(row.complexity.as_deref())),
            ("tags", texts(row.tags.iter().map(String::as_str))),
            ("confidence", optional_float(row.confidence.map(|c| c.0))),
            ("rationale", optional_text(row.rationale.as_deref())),
            ("validation_status", text(&row.validation_status)),
            ("error", optional_text(row.error.as_deref())),
            ("duration_ms", optional_int(row.duration_ms)),
            ("is_current", Value::Bool(row.is_current)),
            ("confirmed_by", optional_text(row.confirmed_by.as_deref())),
            ("batch_id", optional_id(row.batch_id)),
            ("created_by", text(&row.created_by)),
            ("created_at", timestamp(row.created_at, TimeFormat::ModelZ)),
        ],
    )
}
