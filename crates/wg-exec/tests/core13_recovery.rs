//! CORE-13: each worktree recovery action follows `WorktreeManager.recover` in
//! `integrations/git/worktrees.py`.

use wg_exec::recovery::{RecoveryAction, WorktreeFacts, recovery_action};
use wg_types::{AttemptStatus as A, WorktreeStatus as W};

fn live(status: W) -> WorktreeFacts {
    WorktreeFacts {
        status,
        exists_on_disk: true,
        attempt_status: Some(A::Running),
        task_closed: false,
        stack_ended: false,
    }
}

#[test]
fn missing_checkout_is_orphaned_even_when_finished() {
    let facts = WorktreeFacts {
        exists_on_disk: false,
        attempt_status: Some(A::Succeeded),
        ..live(W::Active)
    };
    assert_eq!(recovery_action(facts), RecoveryAction::Orphan);
}

#[test]
fn finished_attempt_releases_the_row() {
    for status in [
        A::Succeeded,
        A::Failed,
        A::Cancelled,
        A::TimedOut,
        A::Abandoned,
    ] {
        let facts = WorktreeFacts {
            attempt_status: Some(status),
            ..live(W::Active)
        };
        assert_eq!(recovery_action(facts), RecoveryAction::Release, "{status}");
    }
}

#[test]
fn closed_task_or_ended_stack_releases_the_row() {
    let closed = WorktreeFacts {
        task_closed: true,
        ..live(W::Preparing)
    };
    assert_eq!(recovery_action(closed), RecoveryAction::Release);
    let ended = WorktreeFacts {
        stack_ended: true,
        attempt_status: None,
        ..live(W::Active)
    };
    assert_eq!(recovery_action(ended), RecoveryAction::Release);
}

#[test]
fn preparing_row_with_live_work_is_activated() {
    assert_eq!(
        recovery_action(live(W::Preparing)),
        RecoveryAction::Activate
    );
}

#[test]
fn active_row_with_live_work_is_kept() {
    assert_eq!(recovery_action(live(W::Active)), RecoveryAction::Keep);
}

#[test]
fn rows_that_are_not_preparing_or_active_are_untouched() {
    for status in [W::Released, W::Removed, W::Orphaned] {
        let facts = WorktreeFacts {
            exists_on_disk: false,
            ..live(status)
        };
        assert_eq!(recovery_action(facts), RecoveryAction::Keep, "{status}");
    }
}

#[test]
fn stack_worktree_without_an_attempt_stays_active_while_its_stack_is_open() {
    let facts = WorktreeFacts {
        attempt_status: None,
        ..live(W::Active)
    };
    assert_eq!(recovery_action(facts), RecoveryAction::Keep);
}
