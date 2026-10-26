//! Payloads of the `classification.*` events (`classification/service.py`).

use serde::Serialize;
use serde_json::Value;
use wg_types::Id;

use crate::entity::Real;

/// `classification.completed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassificationCompleted {
    pub classification_id: Id,
    pub category: Option<String>,
    pub confidence: Option<Real>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub is_current: bool,
    pub prompt_version: Option<String>,
    pub taxonomy_version: String,
}

/// `classification.confirmed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassificationConfirmed {
    pub classification_id: Id,
    pub category: Option<String>,
    /// `{"category": ..., "model": ...}` of the former current classification, or null.
    pub previous: Option<Value>,
}

/// `classification.disagreement`: the current and the new category, and whether the new one
/// replaced the current one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassificationDisagreement {
    pub current: Value,
    pub new: Value,
    pub replaced: Value,
}

/// `classification.evaluated`. Written against the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassificationEvaluated {
    pub evaluation_id: Id,
    pub provider: String,
    pub model: String,
    pub accuracy: Option<Real>,
    pub sample_size: i64,
}

/// `classification.failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassificationFailed {
    pub classification_id: Id,
    pub reason: String,
    pub error: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// `classification.promoted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassificationPromoted {
    pub classification_id: Id,
    pub previous_id: Option<Id>,
}

/// `classification.superseded`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassificationSuperseded {
    pub previous_id: Option<Id>,
    pub new_id: Id,
    pub reason: String,
}
