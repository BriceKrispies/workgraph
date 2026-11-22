//! The intake stage: tasks agents create start as `proposed` and stay out of the backlog until
//! someone admits, merges or drops them (`task intake`).
//!
//! This module holds the decisions that need no store: whether the workspace puts agent creates
//! into intake (`tasks.intake_proposed`), which tasks an intake action may touch, and the text a
//! merge appends to its target.

use serde_json::{Map, Value};
use wg_model::entity::Task;
use wg_types::{TaskStatus, WgError};

/// Whether tasks created through MCP start as proposed (`tasks.intake_proposed`, default off).
#[must_use]
pub fn intake_enabled(config: &Map<String, Value>) -> bool {
    config
        .get("tasks")
        .and_then(|tasks| tasks.get("intake_proposed"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The status an intake action admits a proposal to: ready, or backlog.
#[must_use]
pub fn admit_status(ready: bool) -> TaskStatus {
    if ready {
        TaskStatus::Ready
    } else {
        TaskStatus::Backlog
    }
}

/// An intake action applies to a proposed task only.
pub fn require_proposed(task: &Task) -> Result<(), WgError> {
    if task.status == TaskStatus::Proposed {
        return Ok(());
    }
    Err(WgError::invalid_transition(format!(
        "task {} is {}, not proposed; intake acts on proposed tasks only",
        task.key, task.status
    ))
    .detail("status", task.status.as_str()))
}

/// The comment a merge appends to the target: the proposal's description and criteria.
#[must_use]
pub fn merge_comment(proposal: &Task) -> String {
    let description = proposal.description.trim();
    let criteria: String = proposal
        .acceptance_criteria
        .iter()
        .map(|criterion| format!("\n- {criterion}"))
        .collect();
    let mut body = format!("Merged from proposal {}: {}", proposal.key, proposal.title);
    if !description.is_empty() {
        body.push_str("\n\n");
        body.push_str(description);
    }
    if !criteria.is_empty() {
        body.push_str("\n\nAcceptance criteria:");
        body.push_str(&criteria);
    }
    body
}

/// The reason recorded on a merged proposal when it is cancelled.
#[must_use]
pub fn merged_reason(target_key: &str) -> String {
    format!("merged into {target_key} at intake")
}

/// The reason recorded on a dropped proposal when it is cancelled.
#[must_use]
pub fn dropped_reason(reason: &str) -> String {
    format!("dropped at intake: {reason}")
}
