//! The `task_contract_check` output: the dict `ContractVerdict.as_dict` builds (`verdict`,
//! `reasons`, `missing`, `prediction`, `target`, `model`).
//!
//! It is a plain dict in Python, so it has no class name and is not listed in
//! [`super::implemented`]. The nested prediction is the dump of a `PredictionOut`, whose
//! timestamps print in the `Z` form.

use wg_json::{Value, dict};

use super::predictor::{self, Prediction};
use crate::convention::{text, texts};

/// A contract verdict, as the contract checker decides it.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractVerdict {
    /// One of `not_delegable`, `incomplete`, `split`, `leaf`.
    pub verdict: String,
    pub reasons: Vec<String>,
    pub missing: Vec<String>,
    pub prediction: Option<Prediction>,
    pub target: f64,
    pub model: String,
}

/// The verdict as its dict, keys in Python's order.
pub fn verdict(check: &ContractVerdict) -> Value {
    dict!({
        "verdict": text(&check.verdict),
        "reasons": texts(check.reasons.iter().map(String::as_str)),
        "missing": texts(check.missing.iter().map(String::as_str)),
        "prediction": check
            .prediction
            .as_ref()
            .map_or(Value::Null, predictor::prediction),
        "target": Value::Float(check.target),
        "model": text(&check.model),
    })
}
