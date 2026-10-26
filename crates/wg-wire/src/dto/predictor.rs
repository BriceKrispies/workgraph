//! The predictor's outputs: `PredictorFitOut` (a stored fit, as `experiment_fit` returns it) and
//! `PredictionOut` (a success probability, as `experiment_predict` and the contract check return
//! it), both Pydantic models of `core/models/schemas.py`.
//!
//! Neither is served over HTTP, so neither is listed in [`super::implemented`]. Pydantic datetimes
//! print in the `Z` form. The coefficient, penalty and sample lists are `list[float]`, so each
//! stored number prints as a float even when it is integral.

use serde_json::Value as Json;
use wg_json::Value;
use wg_model::entity::PredictorFit;
use wg_types::{Id, Timestamp};

use crate::convention::{
    TimeFormat, id, int, json_object, model, optional_float, optional_text, optional_timestamp,
    text, texts, timestamp,
};

/// The Python class of the fit DTO.
pub const FIT_NAME: &str = "PredictorFitOut";
/// The fields of [`FIT_NAME`] in declaration order.
pub const FIT_FIELDS: [&str; 14] = [
    "id",
    "workspace_id",
    "model",
    "created_at",
    "data_cutoff",
    "trial_ids",
    "feature_names",
    "repositories",
    "coef",
    "penalty",
    "n",
    "converged",
    "metrics",
    "samples",
];

/// The Python class of the prediction DTO.
pub const PREDICTION_NAME: &str = "PredictionOut";
/// The fields of [`PREDICTION_NAME`] in declaration order.
pub const PREDICTION_FIELDS: [&str; 10] = [
    "probability",
    "low",
    "high",
    "fit_id",
    "model",
    "data_cutoff",
    "n",
    "repository",
    "repository_known",
    "inference",
];

/// A stored predictor fit as its DTO.
pub fn fit_from_model(fit: &PredictorFit) -> Value {
    model(
        FIT_NAME,
        vec![
            ("id", id(fit.id)),
            ("workspace_id", id(fit.workspace_id)),
            ("model", text(&fit.model)),
            ("created_at", timestamp(fit.created_at, TimeFormat::ModelZ)),
            (
                "data_cutoff",
                optional_timestamp(fit.data_cutoff, TimeFormat::ModelZ),
            ),
            ("trial_ids", texts(fit.trial_ids.iter().map(String::as_str))),
            (
                "feature_names",
                texts(fit.feature_names.iter().map(String::as_str)),
            ),
            (
                "repositories",
                texts(fit.repositories.iter().map(String::as_str)),
            ),
            ("coef", floats(fit.coef.iter())),
            ("penalty", floats(fit.penalty.iter())),
            ("n", int(fit.n)),
            ("converged", Value::Bool(fit.converged)),
            ("metrics", json_object(&fit.metrics)),
            (
                "samples",
                Value::List(
                    fit.samples
                        .iter()
                        .map(|row| match row {
                            Json::Array(items) => floats(items.iter()),
                            other => float(other),
                        })
                        .collect(),
                ),
            ),
        ],
    )
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

/// A [`Prediction`] as its DTO.
pub fn prediction(prediction: &Prediction) -> Value {
    model(
        PREDICTION_NAME,
        vec![
            ("probability", Value::Float(prediction.probability)),
            ("low", optional_float(prediction.low)),
            ("high", optional_float(prediction.high)),
            ("fit_id", id(prediction.fit_id)),
            ("model", text(&prediction.model)),
            (
                "data_cutoff",
                optional_timestamp(prediction.data_cutoff, TimeFormat::ModelZ),
            ),
            ("n", int(prediction.n)),
            (
                "repository",
                optional_text(prediction.repository.as_deref()),
            ),
            ("repository_known", Value::Bool(prediction.repository_known)),
            ("inference", Value::Bool(true)),
        ],
    )
}

/// A JSON number as a `float`; anything else is `null`.
fn float(value: &Json) -> Value {
    value.as_f64().map_or(Value::Null, Value::Float)
}

/// A `list[float]` from stored JSON numbers.
fn floats<'a>(values: impl IntoIterator<Item = &'a Json>) -> Value {
    Value::List(values.into_iter().map(float).collect())
}
