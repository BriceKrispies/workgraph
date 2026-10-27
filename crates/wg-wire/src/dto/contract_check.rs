//! The `task_contract_check` output: the dict `ContractVerdict.as_dict` builds (`verdict`,
//! `reasons`, `missing`, `prediction`, `target`, `model`).
//!
//! It is a plain dict in Python, so it has no class name and is not listed in
//! [`super::implemented`]. The nested prediction is the dump of a `PredictionOut`, whose
//! timestamps print in the `Z` form.

use wg_json::Value;

use super::predictor::{self, Prediction};
use crate::wire_dto;

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

wire_dto! {
    /// The verdict as its dict, keys in Python's order.
    pub struct VerdictOut<'a>("ContractVerdict") dict verdict(c: &'a ContractVerdict) {
        verdict: &'a str = &c.verdict,
        reasons: &'a [String] = &c.reasons,
        missing: &'a [String] = &c.missing,
        prediction: Option<Value> = c.prediction.as_ref().map(predictor::prediction),
        target: f64 = c.target,
        model: &'a str = &c.model,
    }
}
