//! Payloads of the `verification.*` events (`verification/service.py`).

use serde::Serialize;
use serde_json::Value;
use wg_types::Id;

/// `verification.config_created` and `verification.config_updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerificationConfigWritten {
    pub checks: Value,
    pub name: String,
    pub scope_id: Id,
    pub scope_type: String,
}

/// `verification.config_enabled` and `verification.config_disabled`: no data (`{}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct VerificationConfigToggled {}

/// `verification.started`. `stack` is present only for stack verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerificationStarted {
    pub checks: Value,
    pub manifest_id: Option<Id>,
    pub task_id: Id,
    pub task_key: String,
    pub tier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<Id>,
}

/// `verification.completed`. `stack` is present only for stack verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerificationCompleted {
    pub failed_checks: Value,
    pub optional_failures: Value,
    pub required_passed: bool,
    pub status: String,
    pub task_id: Id,
    pub task_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack: Option<Id>,
}
