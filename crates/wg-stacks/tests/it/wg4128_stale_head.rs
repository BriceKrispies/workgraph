//! WG-4128: an item whose attempt branch moved past the head recorded at submit time must not merge
//! that stale head. The integration refuses the item's merge with the reason, and the note is
//! cleared once the head merges.

use wg_stacks::merge::{ItemStep, guard_stale, stale_head_reason};
use wg_stacks::queue::{ItemDetail, STALE_KEY, set_stale};

#[test]
fn a_merge_is_refused_only_when_the_branch_moved() {
    let reason = "why".to_string();
    assert_eq!(
        guard_stale(ItemStep::Merge("h2".into()), Some(reason.clone())),
        ItemStep::Refuse(reason.clone())
    );
    // Without a stale reason the merge proceeds as before.
    assert_eq!(
        guard_stale(ItemStep::Merge("h2".into()), None),
        ItemStep::Merge("h2".into())
    );
    // Steps that merge nothing stay as they are.
    for step in [ItemStep::MergeUnchanged, ItemStep::Skip, ItemStep::Noop] {
        assert_eq!(guard_stale(step.clone(), Some(reason.clone())), step);
    }
}

#[test]
fn an_advanced_branch_names_both_commits() {
    assert_eq!(
        stale_head_reason("wg/wg-1/attempt-1", "h1", "h2", true),
        "attempt branch wg/wg-1/attempt-1 advanced from h1 to h2; adopt or resubmit the attempt so the new head runs the submit gate"
    );
}

#[test]
fn a_rewritten_branch_says_its_history_changed() {
    let reason = stale_head_reason("wg/wg-1/attempt-1", "h1", "h9", false);
    assert!(reason.contains("was rewritten"), "{reason}");
    assert!(reason.contains("h9") && reason.contains("h1"), "{reason}");
    assert!(reason.ends_with("resubmit the attempt so the new head runs the submit gate"));
}

#[test]
fn the_stale_note_is_set_once_and_cleared_per_repository() {
    let mut detail = ItemDetail::default();
    assert!(set_stale(&mut detail, "skein", Some("moved")));
    assert!(!set_stale(&mut detail, "skein", Some("moved")));
    assert_eq!(detail.other[STALE_KEY]["skein"], "moved");

    assert!(set_stale(&mut detail, "skein", None));
    assert!(!detail.other.contains_key(STALE_KEY));
    assert!(!set_stale(&mut detail, "skein", None));
}
