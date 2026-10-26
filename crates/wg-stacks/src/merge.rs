//! Merge plans and conflicts for stack integration. Mirrors `StackIntegrator._integrate_item`,
//! `merge_head`, `record_conflict`, `fully_merged`, `start_integrating` and `directory_name` in
//! `integrations/git/stack_integration.py`. Running git and writing rows stay in the shell.

use serde_json::{Value, json};
use wg_types::{StackStatus, TaskStatus};

use crate::ejection::returns_to_ready;
use crate::queue::ItemDetail;

/// Integration merges are commits made by Workgraph itself: (variable, value) pairs for git's
/// author and committer environment.
pub const INTEGRATION_IDENTITY: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Workgraph Stack Integration"),
    ("GIT_AUTHOR_EMAIL", "integration@workgraph.local"),
    ("GIT_COMMITTER_NAME", "Workgraph Stack Integration"),
    ("GIT_COMMITTER_EMAIL", "integration@workgraph.local"),
];

/// `directory_name`: every character outside `[A-Za-z0-9._-]` becomes `_`.
pub fn directory_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// What to do with one due item for one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemStep {
    /// No repository changed: the item is integrated as it is.
    MergeUnchanged,
    /// The item changed no file in this repository.
    Skip,
    /// This repository already merged the item's current head.
    Noop,
    /// Merge this head.
    Merge(String),
}

/// The decision at the top of `_integrate_item`.
///
/// `any_head` is whether the task left a head in any stack repository, `head` the head in this
/// repository and `merged_head` the head already recorded in the item's merges for it.
pub fn plan_item(any_head: bool, head: Option<&str>, merged_head: Option<&str>) -> ItemStep {
    if !any_head {
        return ItemStep::MergeUnchanged;
    }
    match head {
        None => ItemStep::Skip,
        Some(h) if merged_head == Some(h) => ItemStep::Noop,
        Some(h) => ItemStep::Merge(h.to_string()),
    }
}

/// The merge commit message.
pub fn merge_message(task_key: &str, title: &str, branch: &str) -> String {
    format!("Merge {task_key}: {title} into {branch}")
}

/// Arguments of the git merge `merge_head` runs.
pub fn merge_args(head: &str, message: &str) -> Vec<String> {
    ["merge", "--no-ff", "--no-edit", "-m", message, head]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The result of merging one head into an integration worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub merge_commit: String,
    /// `None` when the merge was made or the head was already contained in the branch.
    pub conflict_paths: Option<Vec<String>>,
    pub git_error: String,
    pub already_integrated: bool,
}

impl MergeOutcome {
    /// The head was already an ancestor of the tip: nothing to merge.
    pub fn already_integrated(tip: &str) -> Self {
        Self {
            merge_commit: tip.into(),
            conflict_paths: None,
            git_error: String::new(),
            already_integrated: true,
        }
    }

    /// The merge succeeded and produced `merged`.
    pub fn merged(merged: &str) -> Self {
        Self {
            merge_commit: merged.into(),
            conflict_paths: None,
            git_error: String::new(),
            already_integrated: false,
        }
    }

    /// The merge failed and was aborted: `unmerged` is the output of
    /// `git diff --name-only --diff-filter=U`, `stderr` git's error text (cut to 500 characters).
    pub fn conflicted(tip_before: &str, unmerged: &str, stderr: &str) -> Self {
        Self {
            merge_commit: tip_before.into(),
            conflict_paths: Some(
                unmerged
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect(),
            ),
            git_error: stderr.trim().chars().take(500).collect(),
            already_integrated: false,
        }
    }
}

/// Records a successful merge in the item's detail and clears that repository's conflict.
pub fn record_merge(detail: &mut ItemDetail, repository: &str, head: &str, merge_commit: &str) {
    detail.merges.insert(
        repository.into(),
        json!({ "head": head, "merge_commit": merge_commit }),
    );
    detail.conflicts.remove(repository);
}

/// `fully_merged`: every repository the item changed has merged its current head. `heads` pairs
/// repository names with the task's current head there.
pub fn fully_merged(detail: &ItemDetail, heads: &[(String, String)]) -> bool {
    heads.iter().all(|(name, head)| {
        detail
            .merges
            .get(name)
            .and_then(|m| m.get("head"))
            .and_then(Value::as_str)
            == Some(head)
    })
}

/// The open conflict recorded for a repository; also the event data and comment data.
pub fn conflict_record(
    repository: &str,
    branch: &str,
    head: &str,
    outcome: &MergeOutcome,
) -> Value {
    let paths = outcome.conflict_paths.clone().unwrap_or_default();
    let tip = &outcome.merge_commit;
    let git_error = if paths.is_empty() {
        outcome.git_error.as_str()
    } else {
        ""
    };
    json!({
        "repository": repository,
        "head": head,
        "paths": paths,
        "integration_tip": tip,
        "instruction": format!(
            "rebase this change onto the integration tip {tip} of {branch} and resolve the conflicts there"
        ),
        "git_error": git_error,
    })
}

/// Whether a conflicting child's task returns to `ready` (same statuses as an ejection).
pub fn conflict_returns_task(status: TaskStatus) -> bool {
    returns_to_ready(status)
}

/// The reason on the task transition after a conflict.
pub fn conflict_reason(repository: &str) -> String {
    format!("integration conflict in {repository}")
}

/// The comment left on a child whose merge conflicted.
pub fn conflict_comment(
    task_key: &str,
    branch: &str,
    repository: &str,
    conflict: &Value,
) -> String {
    let paths: Vec<&str> = conflict["paths"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let listed = if paths.is_empty() {
        "none reported".to_string()
    } else {
        paths.join(", ")
    };
    format!(
        "Merge conflict integrating {task_key} into {branch} ({repository}). Conflicting paths: {listed}. \
         Integration tip: {}. Next step: {}.",
        conflict["integration_tip"].as_str().unwrap_or_default(),
        conflict["instruction"].as_str().unwrap_or_default(),
    )
}

/// `start_integrating`: the status a stack moves to when its branch is about to change, or `None`
/// when it stays as it is. A verified stack is no longer verified once a child lands.
pub fn start_integrating(status: StackStatus) -> Option<StackStatus> {
    matches!(status, StackStatus::Open | StackStatus::Verified).then_some(StackStatus::Integrating)
}

/// Whether a stack in `status` is history (`INACTIVE_STACK_STATUSES`): integration does nothing.
pub fn is_inactive(status: StackStatus) -> bool {
    matches!(status, StackStatus::Landed | StackStatus::Abandoned)
}
