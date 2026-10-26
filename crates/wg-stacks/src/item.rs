//! Stack item state machine. Mirrors `core/policies/stacks.py`.
//!
//! Items track a child task's place in its parent's stack. They are deliberately not task
//! lifecycle states: a child stays `awaiting_verification` while its item moves through the stack.
//!
//! Target lists are `match` arms over the source state, as in `wg-tasks::lifecycle`. Error details
//! list allowed targets sorted by wire string, as Python's `sorted(...)` does.

use serde_json::{Value, json};
use wg_types::{ErrorCode, StackItemState, WgError};

use StackItemState as S;

/// Targets of each item state, as `STACK_ITEM_TRANSITIONS` in the Python module.
pub fn item_targets(from: S) -> &'static [S] {
    match from {
        S::Pending => &[S::Ready],
        S::Ready => &[S::Merged, S::Conflict, S::Pending],
        S::Merged => &[S::Verified, S::Ejected, S::Ready, S::Conflict, S::Pending],
        S::Conflict => &[S::Ready, S::Pending, S::Ejected],
        S::Ejected => &[S::Pending, S::Ready],
        S::Verified => &[S::Ready, S::Ejected, S::Pending, S::Conflict],
    }
}

/// Every item state in wire-string order, so allowed-target lists come out sorted.
const ITEM_STATE_BY_WIRE: [S; 6] = [
    S::Conflict,
    S::Ejected,
    S::Merged,
    S::Pending,
    S::Ready,
    S::Verified,
];

/// Items that are no longer on the integration branch and may therefore leave the stack.
pub const REMOVABLE_STATES: &[S] = &[S::Pending, S::Ready, S::Conflict, S::Ejected];

/// Items whose merges sit on the integration branch (`ON_BRANCH_STATES` in `stack_verification.py`).
pub const ON_BRANCH_STATES: &[S] = &[S::Merged, S::Verified, S::Conflict];

/// Inside a stack a sibling dependency is satisfied once its item is integration-ready or later
/// (`STACK_DEPENDENCY_SATISFIED_STATES`).
pub const STACK_DEPENDENCY_SATISFIED_STATES: &[S] = &[S::Ready, S::Merged, S::Verified];

/// Checks one item state change. A no-op (same state) is always allowed.
pub fn check_item_transition(current: S, target: S) -> Result<(), WgError> {
    if current == target {
        return Ok(());
    }
    let targets = item_targets(current);
    if targets.contains(&target) {
        return Ok(());
    }
    let allowed: Vec<Value> = ITEM_STATE_BY_WIRE
        .iter()
        .filter(|state| targets.contains(*state))
        .map(|state| json!(state.as_str()))
        .collect();
    Err(WgError::new(
        ErrorCode::InvalidTransition,
        format!("cannot move stack item from {current} to {target}"),
    )
    .with_detail("from", json!(current.as_str()))
    .with_detail("to", json!(target.as_str()))
    .with_detail("allowed", Value::Array(allowed)))
}

/// Whether an item in `state` may leave its stack.
pub fn is_removable(state: S) -> bool {
    REMOVABLE_STATES.contains(&state)
}

/// Whether an item in `state` has its merges on the integration branch.
pub fn is_on_branch(state: S) -> bool {
    ON_BRANCH_STATES.contains(&state)
}

/// Whether an item in `state` satisfies a stack-sibling dependency.
pub fn satisfies_stack_dependency(state: S) -> bool {
    STACK_DEPENDENCY_SATISFIED_STATES.contains(&state)
}

/// The merge order an item carries after a transition to `target`.
///
/// Merge order is the order in which an item first reaches the integration branch; it survives
/// later re-merges. `stack_max` is the largest merge order already recorded in the stack, so a
/// first merge gets `stack_max + 1` (or 1 when nothing has merged yet).
pub fn merge_order_after(current: Option<i64>, target: S, stack_max: Option<i64>) -> Option<i64> {
    match (current, target) {
        (None, S::Merged) => Some(stack_max.unwrap_or(0) + 1),
        _ => current,
    }
}
