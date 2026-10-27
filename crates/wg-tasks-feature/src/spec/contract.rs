//! The `task contract-check` wire shapes (Pure): the prediction a forecast makes and the verdict
//! that carries the decision and the forecast it was made with.

use wg_clispec::failure::python_number_text;
use wg_clispec::{CliFailure, Usage};
use wg_experiments::contract::ContractVerdict as Decided;
use wg_experiments::predictor::LeafForecast;
use wg_types::{Id, Timestamp};
use wg_wire::dto::contract_check::ContractVerdict;
use wg_wire::dto::predictor::Prediction;

/// The usage of `task contract-check`, as Click prints it in a usage error.
const CONTRACT_CHECK: Usage = Usage {
    usage: "workgraph task contract-check [OPTIONS] {ref}",
    command: "workgraph task contract-check",
};

/// `--target` as typer reads it: a float, defaulting to 0.8 when the option is absent.
///
/// # Errors
///
/// Returns the usage error Click prints when the text is not a float.
pub fn contract_target(text: Option<String>) -> Result<f64, CliFailure> {
    match text {
        None => Ok(0.8),
        Some(text) => python_number_text(&text)
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(|| CONTRACT_CHECK.invalid_value("--target", &text, "float")),
    }
}

/// The `PredictionOut` of a forecast from fit `fit_id` of `model`.
#[must_use]
pub fn prediction_out(
    forecast: LeafForecast,
    fit_id: Id,
    model: &str,
    data_cutoff: Option<Timestamp>,
    n: i64,
    repository: Option<String>,
) -> Prediction {
    Prediction {
        probability: forecast.probability,
        low: forecast.low,
        high: forecast.high,
        fit_id,
        model: model.to_owned(),
        data_cutoff,
        n,
        repository,
        repository_known: forecast.repository_known,
    }
}

/// The verdict of a decision. The prediction is shown only when the decision made one.
#[must_use]
pub fn verdict_out(
    decided: Decided,
    predicted: Option<Prediction>,
    model: &str,
    target: f64,
) -> ContractVerdict {
    ContractVerdict {
        verdict: decided.verdict.as_str().to_owned(),
        reasons: decided.reasons,
        missing: decided.missing,
        prediction: decided.prediction.and(predicted),
        target,
        model: model.to_owned(),
    }
}
