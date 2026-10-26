//! Worktree crash recovery (CORE-13), mirroring `WorktreeManager.recover` in
//! `integrations/git/worktrees.py`.
//!
//! The shell gathers the facts about one managed worktree row (whether its checkout exists on disk,
//! its attempt's status, whether its task is closed, whether its stack ended). This module decides
//! what recovery does with the row. Rows that are neither preparing nor active are not recovered.

use wg_types::{AttemptStatus, WorktreeStatus};

use crate::attempt::is_terminal;

/// The facts about one managed worktree row that recovery decides from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorktreeFacts {
    pub status: WorktreeStatus,
    /// The checkout exists on disk and is a Git repository.
    pub exists_on_disk: bool,
    /// The attempt that owns the worktree, if any, and its status.
    pub attempt_status: Option<AttemptStatus>,
    /// The attempt's task was accepted or cancelled, or archived.
    pub task_closed: bool,
    /// The worktree belongs to a stack that landed or was abandoned.
    pub stack_ended: bool,
}

/// What recovery does with a worktree row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// The checkout is missing: the row becomes orphaned.
    Orphan,
    /// The work it belongs to has ended: the row is released.
    Release,
    /// A preparing row whose checkout exists and whose work is live: it becomes active.
    Activate,
    /// Nothing changes.
    Keep,
}

/// Decides recovery for one worktree row.
pub fn recovery_action(facts: WorktreeFacts) -> RecoveryAction {
    if !matches!(
        facts.status,
        WorktreeStatus::Preparing | WorktreeStatus::Active
    ) {
        return RecoveryAction::Keep;
    }
    let finished =
        facts.attempt_status.is_some_and(is_terminal) || facts.task_closed || facts.stack_ended;
    if !facts.exists_on_disk {
        RecoveryAction::Orphan
    } else if finished {
        RecoveryAction::Release
    } else if facts.status == WorktreeStatus::Preparing {
        RecoveryAction::Activate
    } else {
        RecoveryAction::Keep
    }
}
