//! The automation family: `automation_rules`, `automation_runs`, `notifications`, `reports` and
//! `findings`.

use rpds::VectorSync;
use wg_types::{Correlation, Id, Timestamp};

use crate::entity::{JsonObject, Real};
use crate::write::{Row, RowKey};

/// A row of `automation_rules`: a trigger, its conditions and the actions it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRule {
    pub id: Id,
    pub workspace_id: Id,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    /// The `trigger` column (a JSON object).
    pub trigger: JsonObject,
    pub conditions: JsonObject,
    /// JSON list of action objects.
    pub actions: VectorSync<JsonObject>,
    pub retry_max_attempts: i64,
    pub max_depth: i64,
    pub last_scheduled_at: Option<Timestamp>,
    pub next_run_at: Option<Timestamp>,
    pub created_by: String,
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// A row of `automation_runs`: one evaluation of a rule for one trigger. Unique per rule and
/// trigger key, which makes a trigger idempotent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRun {
    pub id: Id,
    pub rule_id: Id,
    pub workspace_id: Id,
    pub trigger_type: String,
    pub trigger_key: String,
    pub trigger_event_id: Option<Id>,
    pub depth: i64,
    pub dry_run: bool,
    pub status: String,
    pub matched: bool,
    pub reason: String,
    /// JSON list of condition trace objects.
    pub condition_trace: VectorSync<JsonObject>,
    /// JSON list of action result objects.
    pub action_results: VectorSync<JsonObject>,
    pub error: Option<String>,
    pub attempts: i64,
    pub correlation_id: Option<Correlation>,
    pub created_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub inputs: JsonObject,
}

/// A row of `notifications`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: Id,
    pub workspace_id: Id,
    pub level: String,
    pub title: String,
    pub body: String,
    pub source: String,
    pub data: JsonObject,
    pub read_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

/// A row of `reports`: a generated report, its scope and its data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub id: Id,
    pub workspace_id: Id,
    pub kind: String,
    pub title: String,
    pub scope: JsonObject,
    pub content_markdown: String,
    pub data: JsonObject,
    pub narrative_source: String,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// A row of `findings`: an analytics finding, identified by its fingerprint within a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub id: Id,
    pub workspace_id: Id,
    pub kind: String,
    pub fingerprint: String,
    pub title: String,
    pub severity: String,
    pub metric_value: Option<Real>,
    pub sample_size: i64,
    pub evidence: JsonObject,
    pub first_detected_at: Timestamp,
    pub last_detected_at: Timestamp,
}

impl AutomationRule {
    pub fn row_key(&self) -> RowKey {
        RowKey::AutomationRule(self.id)
    }
}

impl AutomationRun {
    pub fn row_key(&self) -> RowKey {
        RowKey::AutomationRun(self.id)
    }
}

impl Notification {
    pub fn row_key(&self) -> RowKey {
        RowKey::Notification(self.id)
    }
}

impl Report {
    pub fn row_key(&self) -> RowKey {
        RowKey::Report(self.id)
    }
}

impl Finding {
    pub fn row_key(&self) -> RowKey {
        RowKey::Finding(self.id)
    }
}

impl From<AutomationRule> for Row {
    fn from(row: AutomationRule) -> Self {
        Row::AutomationRule(row)
    }
}

impl From<AutomationRun> for Row {
    fn from(row: AutomationRun) -> Self {
        Row::AutomationRun(row)
    }
}

impl From<Notification> for Row {
    fn from(row: Notification) -> Self {
        Row::Notification(row)
    }
}

impl From<Report> for Row {
    fn from(row: Report) -> Self {
        Row::Report(row)
    }
}

impl From<Finding> for Row {
    fn from(row: Finding) -> Self {
        Row::Finding(row)
    }
}
