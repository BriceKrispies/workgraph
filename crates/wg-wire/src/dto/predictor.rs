//! The predictor's outputs: `PredictorFitOut` (a stored fit, as `experiment_fit` returns it) and
//! `PredictionOut` (a success probability, as `experiment_predict` and the contract check return
//! it), both Pydantic models of `core/models/schemas.py`.
//!
//! Neither is served over HTTP, so neither is listed in [`super::implemented`]. Pydantic datetimes
//! print in the `Z` form. The coefficient, penalty and sample lists are `list[float]`, so each
//! stored number prints as a float even when it is integral.

use rpds::VectorSync;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value as Json};
use wg_json::Value;
use wg_model::entity::PredictorFit;
use wg_types::{Id, Timestamp};

use crate::convention::{json_map, model_z};
use crate::wire_dto;

wire_dto! {
    /// A stored predictor fit as its DTO.
    pub struct PredictorFitOut<'a>("PredictorFitOut") value fit_from_model(f: &'a PredictorFit) {
        id: Id = f.id,
        workspace_id: Id = f.workspace_id,
        model: &'a str = &f.model,
        #[serde(serialize_with = "model_z")]
        created_at: Timestamp = f.created_at,
        #[serde(serialize_with = "model_z")]
        data_cutoff: Option<Timestamp> = f.data_cutoff,
        trial_ids: &'a VectorSync<String> = &f.trial_ids,
        feature_names: &'a VectorSync<String> = &f.feature_names,
        repositories: &'a VectorSync<String> = &f.repositories,
        #[serde(serialize_with = "floats_field")]
        coef: &'a VectorSync<Json> = &f.coef,
        #[serde(serialize_with = "floats_field")]
        penalty: &'a VectorSync<Json> = &f.penalty,
        n: i64 = f.n,
        converged: bool = f.converged,
        #[serde(serialize_with = "json_map")]
        metrics: &'a Map<String, Json> = &f.metrics,
        #[serde(serialize_with = "samples_field")]
        samples: &'a VectorSync<Json> = &f.samples,
    }
}

/// A success probability from a fit, as the predictor builds it. Its `inference` flag is always
/// true: a prediction is never a measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    pub probability: f64,
    pub low: Option<f64>,
    pub high: Option<f64>,
    pub fit_id: Id,
    pub model: String,
    pub data_cutoff: Option<Timestamp>,
    pub n: i64,
    pub repository: Option<String>,
    pub repository_known: bool,
}

wire_dto! {
    /// A [`Prediction`] as its DTO.
    pub struct PredictionOut<'a>("PredictionOut") value prediction(p: &'a Prediction) {
        probability: f64 = p.probability,
        low: Option<f64> = p.low,
        high: Option<f64> = p.high,
        fit_id: Id = p.fit_id,
        model: &'a str = &p.model,
        #[serde(serialize_with = "model_z")]
        data_cutoff: Option<Timestamp> = p.data_cutoff,
        n: i64 = p.n,
        repository: Option<&'a str> = p.repository.as_deref(),
        repository_known: bool = p.repository_known,
        inference: bool = true,
    }
}

/// A JSON number as a `float`; anything else is `null`.
fn float(value: &Json) -> Value {
    value.as_f64().map_or(Value::Null, Value::Float)
}

/// A `list[float]` from stored JSON numbers.
fn floats<'a>(values: impl IntoIterator<Item = &'a Json>) -> Value {
    Value::List(values.into_iter().map(float).collect())
}

fn floats_field<S: Serializer>(
    values: &&VectorSync<Json>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    floats(values.iter()).serialize(serializer)
}

/// The `samples` rows: each an array of floats, or a single float.
fn samples_field<S: Serializer>(
    rows: &&VectorSync<Json>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    Value::List(
        rows.iter()
            .map(|row| match row {
                Json::Array(items) => floats(items.iter()),
                other => float(other),
            })
            .collect(),
    )
    .serialize(serializer)
}
