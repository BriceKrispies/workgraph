//! Repository identity: the fingerprint of a history, and the rules that match a registration
//! request to an existing repository (`RepositoryService.register` in `repositories.py`).
//!
//! The shell runs the lookups (by checkout path, by fingerprint, by normalised remote) and passes
//! their results in; this module decides which result wins and what name a new repository gets.

use std::collections::BTreeSet;

use wg_model::py_repr_str;
use wg_types::{ErrorCode, Id, WgError};

/// How a registration was matched to an existing repository (the `matched_by` of the event).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    Path,
    Fingerprint,
    Remote,
}

impl MatchedBy {
    /// The wire name used in the `repository.registered` event data.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Fingerprint => "fingerprint",
            Self::Remote => "remote",
        }
    }
}

/// The identity fingerprint of a history: its root-commit SHAs, sorted, unique and joined with
/// commas. `None` when there are no root commits (an empty repository).
pub fn fingerprint(root_commits: &[String]) -> Option<String> {
    let roots: BTreeSet<&str> = root_commits.iter().map(String::as_str).collect();
    if roots.is_empty() {
        None
    } else {
        Some(roots.into_iter().collect::<Vec<_>>().join(","))
    }
}

/// Whether a recorded fingerprint and an observed one describe different histories. Missing or
/// empty values never mismatch (Python's `if fingerprint and roots and roots != fingerprint`).
pub fn history_mismatch(recorded: Option<&str>, observed: Option<&str>) -> bool {
    match (recorded, observed) {
        (Some(recorded), Some(observed)) if !recorded.is_empty() && !observed.is_empty() => {
            recorded != observed
        }
        _ => false,
    }
}

/// The match of a registration, given the repository found by checkout path, by fingerprint and
/// by normalised remote (each `None` when the lookup found nothing or was not run). The first
/// lookup that found something wins, in that order.
pub fn matched_by(
    by_path: Option<Id>,
    by_fingerprint: Option<Id>,
    by_remote: Option<Id>,
) -> Option<(Id, MatchedBy)> {
    by_path
        .map(|id| (id, MatchedBy::Path))
        .or_else(|| by_fingerprint.map(|id| (id, MatchedBy::Fingerprint)))
        .or_else(|| by_remote.map(|id| (id, MatchedBy::Remote)))
}

/// The name for a new repository: the requested name, else the checkout's directory name. A
/// taken explicit name is a conflict; a taken derived name gets the first free `-2`, `-3`, ...
pub fn new_repository_name(
    requested: Option<&str>,
    dir_name: &str,
    taken: &[&str],
) -> Result<String, WgError> {
    let requested = requested.filter(|name| !name.is_empty());
    let name = requested.unwrap_or(dir_name);
    if !taken.contains(&name) {
        return Ok(name.to_owned());
    }
    if requested.is_some() {
        return Err(WgError::new(
            ErrorCode::Conflict,
            format!(
                "repository name {} is already registered",
                py_repr_str(name)
            ),
        ));
    }
    (2_u32..)
        .map(|suffix| format!("{name}-{suffix}"))
        .find(|candidate| !taken.contains(&candidate.as_str()))
        .ok_or_else(|| {
            WgError::new(
                ErrorCode::Conflict,
                format!(
                    "repository name {} is already registered",
                    py_repr_str(name)
                ),
            )
        })
}
