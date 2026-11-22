//! The decisions of the land-review guardrail: when work carrying a task's `Task-ID` trailer is
//! reachable from the repository's integration branch, the task moves to `awaiting_review`.
//! The service reads git and the rows and applies the result; everything it decides is here.
//!
//! A landed commit moves a task only when it is newer than the task's last status change (so a
//! task rejected or reopened after the commit stays where the reviewer put it, and a moved task
//! is not moved again by the same commit) and the task has no child that is not settled.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use wg_types::{Id, Timestamp};

use super::sql::TrailerLanding;

/// The key of the comment data that records which commit moved a task. Informational: the
/// idempotency rule is the commit time against the last status change.
pub const MARKER_KEY: &str = "landed_commit";

/// The task statuses the guardrail moves, as stored.
#[must_use]
pub fn eligible_statuses() -> Vec<&'static str> {
    wg_tasks::lifecycle::LAND_REVIEW_SOURCES
        .iter()
        .map(|status| status.as_str())
        .collect()
}

/// The integration branch to use: the repository's own setting, else the one of its most recent
/// stack. `None` when neither exists, and the guardrail does nothing.
#[must_use]
pub fn integration_branch(
    configured: Option<String>,
    from_stack: Option<String>,
) -> Option<String> {
    configured
        .filter(|name| !name.is_empty())
        .or(from_stack.filter(|name| !name.is_empty()))
}

/// The ref git reads for a branch name.
#[must_use]
pub fn branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

/// For each task, the newest of its trailer commits that the integration branch contains.
/// `landings` come newest first.
#[must_use]
pub fn landed(landings: &[TrailerLanding], reachable: &BTreeSet<String>) -> BTreeMap<Id, String> {
    let mut found = BTreeMap::new();
    for landing in landings {
        if reachable.contains(&landing.sha) {
            found
                .entry(landing.task_id)
                .or_insert_with(|| landing.sha.clone());
        }
    }
    found
}

/// The landed commits that may move their task: the commit's committer time is strictly newer
/// than the task's last status change, and the task is not in `unsettled_parents`. A commit with
/// no known committer time, or a task with no known status change, does not move.
#[must_use]
pub fn movable(
    landed: &BTreeMap<Id, String>,
    committed_at: &BTreeMap<String, Timestamp>,
    changed_at: &BTreeMap<Id, Timestamp>,
    unsettled_parents: &BTreeSet<Id>,
) -> BTreeMap<Id, String> {
    landed
        .iter()
        .filter(|(task_id, sha)| {
            !unsettled_parents.contains(task_id)
                && matches!(
                    (committed_at.get(*sha), changed_at.get(*task_id)),
                    (Some(commit), Some(change)) if commit > change
                )
        })
        .map(|(task_id, sha)| (*task_id, sha.clone()))
        .collect()
}

/// The reason recorded on the status change.
#[must_use]
pub fn reason(sha: &str, branch: &str) -> String {
    format!("commit {sha} landed on {branch}")
}

/// The body of the comment left on the task.
#[must_use]
pub fn comment_body(sha: &str, branch: &str) -> String {
    format!(
        "Moved to awaiting review: commit {sha} with this task's Task-ID trailer is on {branch}."
    )
}

/// The data of that comment: the commit and branch, under the marker key.
#[must_use]
pub fn comment_data(sha: &str, branch: &str) -> Map<String, Value> {
    [
        (MARKER_KEY.to_owned(), json!(sha)),
        ("branch".to_owned(), json!(branch)),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_statuses_are_the_explicit_list() {
        assert_eq!(
            eligible_statuses(),
            ["backlog", "ready", "blocked", "claimed", "running"]
        );
    }

    #[test]
    fn the_repository_setting_wins_over_the_stack_branch() {
        assert_eq!(
            integration_branch(Some("integration".into()), Some("stack".into())).as_deref(),
            Some("integration")
        );
        assert_eq!(
            integration_branch(None, Some("stack".into())).as_deref(),
            Some("stack")
        );
        assert_eq!(integration_branch(Some(String::new()), None), None);
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::parse_iso(&format!("2026-11-22T12:00:{seconds:02}.000000+00:00")).expect("time")
    }

    #[test]
    fn movable_needs_a_newer_commit_and_settled_children() {
        let ids: Vec<Id> = (1..=4).map(Id::from_u128).collect();
        let landed: BTreeMap<Id, String> = ids
            .iter()
            .enumerate()
            .map(|(n, id)| (*id, format!("sha{n}")))
            .collect();
        let committed: BTreeMap<String, Timestamp> = [
            ("sha0".to_owned(), at(10)),
            ("sha1".to_owned(), at(10)),
            ("sha2".to_owned(), at(10)),
        ]
        .into();
        let changed: BTreeMap<Id, Timestamp> = [
            (ids[0], at(5)),
            (ids[1], at(10)),
            (ids[2], at(5)),
            (ids[3], at(5)),
        ]
        .into();
        let unsettled: BTreeSet<Id> = [ids[2]].into();
        let moved = movable(&landed, &committed, &changed, &unsettled);
        assert_eq!(moved.keys().copied().collect::<Vec<_>>(), vec![ids[0]]);
    }
}
