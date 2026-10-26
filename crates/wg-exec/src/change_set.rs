//! Change set rollups (CORE-13), mirroring the status rules in `integrations/git/worktrees.py`
//! (capture of an attempt's outcome) and `verification/service.py` (`_apply_change_set_status`).
//!
//! A change set tracks each repository's item independently; the rollups here derive the change
//! set's status from the item statuses.

use wg_types::{ChangeSetItemStatus as I, ChangeSetStatus as C};

/// The status of a change set after its outcome was captured: `Open` when nothing committed,
/// `PartiallyApplied` when some items committed, `Applied` when every item did.
pub fn applied_status(items: &[I]) -> C {
    let committed = items.iter().filter(|s| **s == I::Committed).count();
    if committed == 0 {
        C::Open
    } else if committed < items.len() {
        C::PartiallyApplied
    } else {
        C::Applied
    }
}

/// An item's status once a verification run finished: committed items become verified or failed,
/// other items keep their status.
pub fn verified_item_status(status: I, passed: bool) -> I {
    match status {
        I::Committed if passed => I::Verified,
        I::Committed => I::Failed,
        other => other,
    }
}

/// The change set's status after a verification run, or `None` when it stays as it is.
///
/// A passing run with every item verified is `Verified`; passing with some verified is
/// `PartiallyVerified`; a failing run is `Failed`. `items` are the statuses after
/// `verified_item_status` was applied to each item.
pub fn verified_status(items: &[I], passed: bool) -> Option<C> {
    let any_verified = items.contains(&I::Verified);
    let all_verified = items.iter().all(|s| *s == I::Verified);
    if passed && all_verified {
        Some(C::Verified)
    } else if passed && any_verified {
        Some(C::PartiallyVerified)
    } else if !passed {
        Some(C::Failed)
    } else {
        None
    }
}
