//! One classification evaluation of `GET /classification/evaluations`: the accuracy of one
//! provider, model and prompt over a workspace's gold labels.

use wg_json::Value;
use wg_model::entity::classification::ClassificationEvaluation;

use crate::convention::{TimeFormat, dict, id, int, json_object, optional_float, text, timestamp};

/// The name of the evaluation dict; it has no OpenAPI component schema.
pub const NAME: &str = "ClassificationEvaluationOut";

/// The fields in the order `ClassificationService.evaluations` builds them.
pub const FIELDS: [&str; 12] = [
    "id",
    "provider",
    "model",
    "prompt_version",
    "taxonomy_version",
    "sample_size",
    "failures",
    "accuracy",
    "macro_f1",
    "per_class",
    "confusion",
    "created_at",
];

/// An evaluation as the route prints it. The service builds it as a dict, so `created_at` is
/// Python's `isoformat()` (`+00:00`), not the model's `Z` form.
pub fn from_model(evaluation: &ClassificationEvaluation) -> Value {
    dict(vec![
        ("id", id(evaluation.id)),
        ("provider", text(&evaluation.provider)),
        ("model", text(&evaluation.model)),
        ("prompt_version", text(&evaluation.prompt_version)),
        ("taxonomy_version", text(&evaluation.taxonomy_version)),
        ("sample_size", int(evaluation.sample_size)),
        ("failures", int(evaluation.failures)),
        ("accuracy", optional_float(evaluation.accuracy.map(|r| r.0))),
        ("macro_f1", optional_float(evaluation.macro_f1.map(|r| r.0))),
        ("per_class", json_object(&evaluation.per_class)),
        ("confusion", json_object(&evaluation.confusion)),
        (
            "created_at",
            timestamp(evaluation.created_at, TimeFormat::Isoformat),
        ),
    ])
}
