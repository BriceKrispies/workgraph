//! Payloads of the `automation.*`, `notification.*` and `report.*` events
//! (`automation/engine.py`, `analytics/retrospective.py`).

use serde::Serialize;
use serde_json::Value;

/// `automation.rule_created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutomationRuleCreated {
    pub name: String,
    pub trigger: Value,
}

/// `automation.rule_updated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutomationRuleUpdated {
    pub version: i64,
}

/// `automation.rule_enabled` and `automation.rule_disabled`: no data (`{}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct AutomationRuleToggled {}

/// `automation.run_completed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutomationRunCompleted {
    pub actions: Value,
    pub error: Option<String>,
    pub rule: String,
    pub status: String,
}

/// `notification.created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotificationCreated {
    pub level: String,
    pub title: String,
}

/// `report.generated`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportGenerated {
    pub kind: String,
    pub narrative_source: String,
}
