//! Repository health: what one checkout's probe means, and the status a repository gets from its
//! checkouts (`RepositoryService.refresh` in `repositories.py`).
//!
//! The shell runs git (is it a repository, its root commits, whether its worktree list marks it
//! prunable) and passes the facts in; this module turns them into the observation and status.

use wg_model::RepositoryCheckout;

use crate::checkout::{KIND_MANAGED_WORKTREE, KIND_WORKTREE};
use crate::identity::history_mismatch;

/// What a probe of one checkout found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    Available,
    /// The path is missing or is not a git repository.
    NotRepository,
    /// The repository here has other root commits than the recorded identity.
    HistoryMismatch,
    /// The worktree is listed as prunable.
    Prunable,
}

impl Observation {
    pub const fn available(self) -> bool {
        matches!(self, Self::Available)
    }

    /// The `error` recorded in the health detail, when there is one.
    pub const fn error(self) -> Option<&'static str> {
        match self {
            Self::Available => None,
            Self::NotRepository => Some("path missing or not a git repository"),
            Self::HistoryMismatch => Some("history does not match repository identity"),
            Self::Prunable => Some("worktree is prunable"),
        }
    }
}

/// The observation of a checkout from its probe: `is_repository` as git reported it, the
/// recorded fingerprint and the observed one (`None` when the history has no root commits).
pub fn observe(
    is_repository: bool,
    recorded_fingerprint: Option<&str>,
    observed_fingerprint: Option<&str>,
) -> Observation {
    if !is_repository {
        Observation::NotRepository
    } else if history_mismatch(recorded_fingerprint, observed_fingerprint) {
        Observation::HistoryMismatch
    } else {
        Observation::Available
    }
}

/// A prunable worktree overrides whatever the probe said, as in Python. Only linked worktrees
/// (`kind == worktree`) at a listed prunable path are affected.
pub fn with_prunable(observation: Observation, kind: &str, prunable_listed: bool) -> Observation {
    if kind == KIND_WORKTREE && prunable_listed {
        Observation::Prunable
    } else {
        observation
    }
}

/// The health status of a repository from its user checkouts (managed worktrees do not count):
/// `ok` when every one is available, `degraded` when some are, `unavailable` when none are. With
/// no user checkout at all the result is `unavailable`.
pub fn health_status(checkouts: &[RepositoryCheckout]) -> &'static str {
    let user: Vec<&RepositoryCheckout> = checkouts
        .iter()
        .filter(|checkout| checkout.kind != KIND_MANAGED_WORKTREE)
        .collect();
    let available = user.iter().filter(|checkout| checkout.available).count();
    if available > 0 && available == user.len() {
        "ok"
    } else if available > 0 {
        "degraded"
    } else {
        "unavailable"
    }
}
