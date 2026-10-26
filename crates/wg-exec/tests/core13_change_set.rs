//! CORE-13: change set rollups follow `_apply_change_set_status` in `verification/service.py` and
//! the capture rule in `integrations/git/worktrees.py`.

use wg_exec::change_set::{applied_status, verified_item_status, verified_status};
use wg_types::{ChangeSetItemStatus as I, ChangeSetStatus as C};

#[test]
fn applied_status_counts_committed_items() {
    assert_eq!(applied_status(&[]), C::Open);
    assert_eq!(applied_status(&[I::Pending, I::Pending]), C::Open);
    assert_eq!(
        applied_status(&[I::Committed, I::Pending]),
        C::PartiallyApplied
    );
    assert_eq!(applied_status(&[I::Committed, I::Committed]), C::Applied);
}

#[test]
fn committed_items_take_the_verification_result() {
    assert_eq!(verified_item_status(I::Committed, true), I::Verified);
    assert_eq!(verified_item_status(I::Committed, false), I::Failed);
    assert_eq!(verified_item_status(I::Pending, true), I::Pending);
    assert_eq!(verified_item_status(I::Pending, false), I::Pending);
    assert_eq!(verified_item_status(I::Verified, false), I::Verified);
}

#[test]
fn passing_run_with_every_item_verified_is_verified() {
    assert_eq!(
        verified_status(&[I::Verified, I::Verified], true),
        Some(C::Verified)
    );
    // Python: an empty item set is a subset of {verified}.
    assert_eq!(verified_status(&[], true), Some(C::Verified));
}

#[test]
fn passing_run_with_some_items_verified_is_partially_verified() {
    assert_eq!(
        verified_status(&[I::Verified, I::Pending], true),
        Some(C::PartiallyVerified)
    );
    assert_eq!(
        verified_status(&[I::Verified, I::Failed], true),
        Some(C::PartiallyVerified)
    );
}

#[test]
fn passing_run_with_no_verified_items_changes_nothing() {
    assert_eq!(verified_status(&[I::Pending, I::Failed], true), None);
}

#[test]
fn failing_run_marks_the_change_set_failed() {
    assert_eq!(
        verified_status(&[I::Failed, I::Verified], false),
        Some(C::Failed)
    );
    assert_eq!(verified_status(&[I::Pending], false), Some(C::Failed));
}
