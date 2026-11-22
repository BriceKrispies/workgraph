//! The decisions of the land-review guardrail: when work carrying a task's `Task-ID` trailer is
//! reachable from the repository's integration branch, the task moves to `awaiting_review`.
//! The service reads git and the rows and applies the result; everything it decides is here.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use wg_types::Id;

use super::sql::TrailerLanding;

/// The key of the comment data that records which commit moved a task, so a task reopened later
/// is not moved again by the same commit.
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

/// Whether a comment's data marks `sha` as already handled.
#[must_use]
pub fn is_marked(data: &Map<String, Value>, sha: &str) -> bool {
    data.get(MARKER_KEY).and_then(Value::as_str) == Some(sha)
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

    #[test]
    fn a_marker_names_one_commit() {
        let data = comment_data("abc", "main");
        assert!(is_marked(&data, "abc"));
        assert!(!is_marked(&data, "def"));
    }
}
