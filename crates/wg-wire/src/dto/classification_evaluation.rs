//! One classification evaluation of `GET /classification/evaluations`: the accuracy of one
//! provider, model and prompt over a workspace's gold labels. A dict with no OpenAPI component
//! schema.

use serde_json::{Map, Value as Json};
use wg_model::entity::classification::ClassificationEvaluation;
use wg_types::{Id, Timestamp};

use crate::convention::{isoformat, json_map};
use crate::wire_dto;

wire_dto! {
    /// An evaluation as the route prints it. The service builds it as a dict, so `created_at` is
    /// Python's `isoformat()` (`+00:00`), not the model's `Z` form.
    pub struct Evaluation<'a>("ClassificationEvaluationOut") dict from_model(e: &'a ClassificationEvaluation) {
        id: Id = e.id,
        provider: &'a str = &e.provider,
        model: &'a str = &e.model,
        prompt_version: &'a str = &e.prompt_version,
        taxonomy_version: &'a str = &e.taxonomy_version,
        sample_size: i64 = e.sample_size,
        failures: i64 = e.failures,
        accuracy: Option<f64> = e.accuracy.map(|r| r.0),
        macro_f1: Option<f64> = e.macro_f1.map(|r| r.0),
        #[serde(serialize_with = "json_map")]
        per_class: &'a Map<String, Json> = &e.per_class,
        #[serde(serialize_with = "json_map")]
        confusion: &'a Map<String, Json> = &e.confusion,
        #[serde(serialize_with = "isoformat")]
        created_at: Timestamp = e.created_at,
    }
}
