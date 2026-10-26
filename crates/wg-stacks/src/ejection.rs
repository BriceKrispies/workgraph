//! Ejection decisions: which record an ejection writes, which items it parks, and the comment it
//! leaves on the child. Mirrors `StackVerifier._eject`, `eject` and `_apply_ejection` in
//! `integrations/git/stack_verification.py`. Marking rows and returning tasks stay in the shell.

use serde_json::{Map, Value, json};
use wg_types::{ErrorCode, StackItemState, TaskStatus, WgError};

use crate::item::is_on_branch;

/// Task statuses an ejection returns to `ready` (`RETURNABLE_TASK_STATUSES`).
pub const RETURNABLE_TASK_STATUSES: &[TaskStatus] =
    &[TaskStatus::AwaitingVerification, TaskStatus::AwaitingReview];

/// Whether an ejection returns a task in `status` to `ready`.
pub fn returns_to_ready(status: TaskStatus) -> bool {
    RETURNABLE_TASK_STATUSES.contains(&status)
}

/// The ejection record of a bisected failure: where bisection placed it, the run, and the checks
/// that failed with their signatures. `signatures` pairs each check name with its signatures, in
/// the order the checks were reported.
pub fn bisected_ejection(
    position: i64,
    run_id: &str,
    signatures: &[(String, Vec<String>)],
) -> Value {
    let mut by_name = Map::new();
    for (name, found) in signatures {
        by_name.insert(name.clone(), json!(found));
    }
    json!({
        "position": position,
        "verification_run_id": run_id,
        "failed_checks": sorted_names(signatures),
        "signatures": Value::Object(by_name),
    })
}

/// The ejection record of an operator's ejection. The reason must name evidence.
pub fn manual_ejection(reason: &str) -> Result<Value, WgError> {
    if reason.trim().is_empty() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "an ejection needs a reason (the evidence that names this child)",
        ));
    }
    Ok(json!({ "manual": true, "reason": reason }))
}

/// The item detail an ejected item carries.
pub fn ejected_detail(ejection: Value) -> Value {
    json!({ "merges": {}, "conflicts": {}, "ejection": ejection })
}

/// The item detail a dependent carries after it is parked because `task_key` was ejected.
pub fn parked_detail(task_key: &str) -> Value {
    json!({
        "merges": {},
        "conflicts": {},
        "reason": format!("dependency {task_key} was ejected"),
    })
}

/// Whether a dependent of an ejected child is parked. Already-ejected dependents are skipped.
pub fn should_park_dependent(state: StackItemState) -> bool {
    state != StackItemState::Ejected
}

/// Checks that an operator's ejection targets a child whose merges are on the integration branch
/// (the culprit must be in the stack's merge sequence). `has_merges` is whether the item's detail
/// records any merges.
pub fn check_ejectable(
    task_key: &str,
    parent_key: &str,
    state: StackItemState,
    has_merges: bool,
) -> Result<(), WgError> {
    if is_on_branch(state) && has_merges {
        return Ok(());
    }
    Err(WgError::new(
        ErrorCode::Conflict,
        format!(
            "{task_key} is not merged on {parent_key}'s stack; only a merged child can be ejected"
        ),
    )
    .with_detail("task", task_key.into()))
}

/// The comment left on a child that bisection ejected.
pub fn bisected_comment(
    parent_key: &str,
    task_key: &str,
    position: i64,
    run_id: &str,
    signatures: &[(String, Vec<String>)],
) -> String {
    let names = sorted_names(signatures).join(", ");
    let listed = signatures
        .iter()
        .map(|(name, found)| format!("{name}: {}", joined_or_none(found)))
        .collect::<Vec<_>>()
        .join("; ");
    let listed = if listed.is_empty() {
        "none reported".to_string()
    } else {
        listed
    };
    format!(
        "Stack {parent_key} verification failed on the integration tip, and bisection placed the failure at \
         {task_key} (position {position} in the merge order). Failing checks: {names}. Failure signatures: \
         {listed}. Verification run: {run_id}. Next step: fix the failure in this change and submit a new \
         attempt; the stack verifies again once the change is integrated."
    )
}

/// The comment left on a child an operator ejected.
pub fn manual_comment(parent_key: &str, actor: &str, reason: &str) -> String {
    format!("Ejected from stack {parent_key} by {actor}: {reason}")
}

fn sorted_names(signatures: &[(String, Vec<String>)]) -> Vec<&str> {
    let mut names: Vec<&str> = signatures.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    names
}

fn joined_or_none(found: &[String]) -> String {
    if found.is_empty() {
        "none reported".to_string()
    } else {
        found.join(", ")
    }
}
