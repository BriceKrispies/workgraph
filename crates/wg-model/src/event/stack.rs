//! Payloads of the `stack.*` events (`core/services/stacks.py`, `integrations/git/stack_*.py`).
//!
//! Lists whose element type is not fixed by the contract (`replayed`, `conflicted`, `released`)
//! are `VectorSync<String>`; dictionaries of repository name to revision are [`StrMap`]; the
//! per-check failure signatures, which are name to list, are [`JsonObject`].

use rpds::VectorSync;
use serde::Serialize;
use wg_types::{Id, StackItemState, StackStatus, TaskKey, TaskStatus};

use crate::entity::JsonObject;
use crate::event::{StrMap, ser};

/// `stack.created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackCreated {
    #[serde(serialize_with = "ser::task_key")]
    pub parent_key: TaskKey,
    pub integration_branch: String,
    pub base_shas: StrMap,
    #[serde(serialize_with = "ser::task_keys")]
    pub tasks: VectorSync<TaskKey>,
}

/// `stack.item_added`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackItemAdded {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    #[serde(serialize_with = "ser::task_key")]
    pub parent_key: TaskKey,
}

/// `stack.item_removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackItemRemoved {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    #[serde(serialize_with = "ser::task_key")]
    pub parent_key: TaskKey,
    pub state: StackItemState,
}

/// `stack.item_released`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackItemReleased {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    #[serde(serialize_with = "ser::task_key")]
    pub parent_key: TaskKey,
    pub state: StackItemState,
    pub rebuild: bool,
}

/// `stack.item_ejected`: the verification evidence that ejected an item (the optional keys are
/// present only when verification produced them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackItemEjected {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_checks: Option<VectorSync<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signatures: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_run_id: Option<Id>,
}

/// `stack.item_state_changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackItemStateChanged {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    #[serde(serialize_with = "ser::task_key")]
    pub parent_key: TaskKey,
    pub from: StackItemState,
    pub to: StackItemState,
    /// The item's detail document, as text.
    pub detail: Option<String>,
    pub merge_order: Option<i64>,
}

/// `stack.status_changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackStatusChanged {
    pub from: StackStatus,
    pub to: StackStatus,
    pub reason: String,
}

/// `stack.land_requested`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackLandRequested {
    #[serde(serialize_with = "ser::task_key")]
    pub parent_key: TaskKey,
    pub mode: String,
    pub reason: String,
    pub base_shas: StrMap,
    pub new_base_shas: StrMap,
}

/// `stack.landed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackLanded {
    #[serde(serialize_with = "ser::task_key")]
    pub parent_key: TaskKey,
    pub mode: String,
    pub reason: String,
    pub run_id: Option<Id>,
    pub tips: StrMap,
    #[serde(serialize_with = "ser::task_keys")]
    pub tasks: VectorSync<TaskKey>,
    #[serde(serialize_with = "ser::task_keys")]
    pub released: VectorSync<TaskKey>,
}

/// `stack.landing_refused`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackLandingRefused {
    pub mode: String,
    pub reason: String,
    pub error: String,
}

/// `stack.rebased`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackRebased {
    #[serde(serialize_with = "ser::task_key")]
    pub parent_key: TaskKey,
    pub reason: String,
    pub base_shas: StrMap,
    pub new_base_shas: StrMap,
}

/// `stack.rebuilt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackRebuilt {
    pub base_shas: StrMap,
    pub tips: StrMap,
    pub replayed: VectorSync<String>,
    pub conflicted: VectorSync<String>,
}

/// `stack.verification_started`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackVerificationStarted {
    pub tips: StrMap,
    pub checks: VectorSync<String>,
}

/// `stack.verified` (`tolerated` is present when the base's failures were tolerated).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackVerified {
    pub run_id: Option<Id>,
    pub tips: StrMap,
    #[serde(serialize_with = "ser::task_keys")]
    pub tasks: VectorSync<TaskKey>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerated: Option<JsonObject>,
}

/// `stack.verification_failed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackVerificationFailed {
    pub run_id: Id,
    pub tips: StrMap,
    pub failed_checks: VectorSync<String>,
    pub signatures: JsonObject,
}

/// `stack.verification_failed_on_base`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackVerificationFailedOnBase {
    pub run_id: Option<Id>,
    pub failed_checks: VectorSync<String>,
    pub reason: String,
}

/// `stack.verification_errored`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackVerificationErrored {
    pub run_id: Id,
    pub stage: String,
    pub checks: VectorSync<String>,
    /// Check name to the tail of its output.
    pub excerpt: StrMap,
    pub reason: String,
}

/// `stack.bisect_probe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackBisectProbe {
    pub position: i64,
    pub shas: StrMap,
    pub passed: bool,
    pub run_id: Id,
    pub failed_checks: VectorSync<String>,
}

/// `stack.integration_merged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackIntegrationMerged {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    pub repository: String,
    pub branch: String,
    pub head: String,
    pub merge_commit: String,
    pub already_integrated: bool,
}

/// `stack.integration_conflict`: the conflict detail (every key of the conflict is optional
/// in the contract's sense, so each is `Option`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackIntegrationConflict {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    pub repository: String,
    pub head: Option<String>,
    pub integration_tip: Option<String>,
    pub paths: VectorSync<String>,
    pub git_error: Option<String>,
    pub instruction: Option<String>,
}

/// `stack.integration_skipped`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackIntegrationSkipped {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    pub repository: String,
    pub reason: String,
}

/// `stack.integration_removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackIntegrationRemoved {
    pub repository: String,
    pub branch: String,
}

/// `stack.parent_not_reviewable`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackParentNotReviewable {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    pub status: TaskStatus,
    pub reason: String,
}

/// `stack.parent_rejected`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackParentRejected {
    #[serde(serialize_with = "ser::task_key")]
    pub task_key: TaskKey,
    pub reason: String,
}
