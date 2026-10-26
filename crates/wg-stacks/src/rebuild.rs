//! Ejection consequences and the rebuild of the integration branch: which items depend on an
//! ejected child, the merges replayed on each repository's base, and how replay results update
//! each item's detail. Port of `StackVerifier.dependents`, `rebuild` and the reason strings of
//! `_apply_ejection` in `integrations/git/stack_verification.py`.
//!
//! The shell resets each integration worktree to its base, runs the merges in the order given here
//! and passes their outcomes back.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use wg_types::WgError;

use crate::bisect::Entry;
use crate::merge::{MergeOutcome, merge_message};
use crate::ordered::{object_with, object_without, sorted_by};
use crate::queue::ItemDetail;

/// The reason an ejected child's task returns to `ready`. `names` are the failing checks, or the
/// operator's reason for a manual ejection.
pub fn ejected_task_reason(parent_key: &str, names: &str) -> String {
    format!("ejected from stack {parent_key}: {names}")
}

/// The failing check names of a bisected ejection, sorted and joined (the reason it records).
pub fn ejection_names(signatures: &[(String, Vec<String>)]) -> String {
    crate::regression::sorted_names(signatures).join(", ")
}

/// One task of the stack for [`dependents`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackTask {
    pub task_id: String,
    pub number: i64,
}

/// `dependents`: the stack's tasks that depend on `task_id`, directly or through other stack
/// tasks, ordered by task number. `edges` are (dependent, prerequisite) pairs; pairs that leave
/// the stack are ignored.
pub fn dependents(tasks: &[StackTask], edges: &[(String, String)], task_id: &str) -> Vec<String> {
    let in_stack: BTreeSet<&str> = tasks.iter().map(|t| t.task_id.as_str()).collect();
    let inner: Vec<(&str, &str)> = edges
        .iter()
        .map(|(dependent, prerequisite)| (dependent.as_str(), prerequisite.as_str()))
        .filter(|(d, p)| in_stack.contains(d) && in_stack.contains(p))
        .collect();
    let reached = reach(&inner, BTreeSet::from([task_id]), BTreeSet::from([task_id]));
    let number = |id: &&str| {
        tasks
            .iter()
            .find(|t| t.task_id == *id)
            .map_or(i64::MAX, |t| t.number)
    };
    sorted_by(reached.into_iter().filter(|id| *id != task_id), number)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Every task reachable from `frontier` along (dependent, prerequisite) edges, plus `seen`.
fn reach<'a>(
    edges: &[(&'a str, &'a str)],
    frontier: BTreeSet<&'a str>,
    seen: BTreeSet<&'a str>,
) -> BTreeSet<&'a str> {
    let next: BTreeSet<&str> = edges
        .iter()
        .filter(|(d, p)| frontier.contains(p) && !seen.contains(d))
        .map(|(d, _)| *d)
        .collect();
    if next.is_empty() {
        return seen;
    }
    let seen = seen.union(&next).copied().collect();
    reach(edges, next, seen)
}

/// One repository's integration target for a rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildTarget {
    pub repository_name: String,
    pub integration_branch: String,
}

/// One merge to replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayMerge {
    pub item_id: String,
    pub task_key: String,
    pub head: String,
    pub message: String,
}

/// The rebuild of one repository: reset to `base_sha`, then merge each head in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRebuild {
    pub repository_name: String,
    pub base_sha: String,
    pub merges: Vec<ReplayMerge>,
}

/// `rebuild`: for each repository, by name, its base and the merges of the items still on the
/// branch, in merge order.
pub fn rebuild_plan(
    sequence: &[Entry],
    targets: &[RebuildTarget],
    bases: &BTreeMap<String, String>,
) -> Result<Vec<RepoRebuild>, WgError> {
    sorted_by(targets.iter(), |target| target.repository_name.clone())
        .into_iter()
        .map(|target| {
            let name = &target.repository_name;
            let merges = sequence
                .iter()
                .filter(|entry| entry.merges.contains_key(name))
                .map(|entry| {
                    Ok(ReplayMerge {
                        item_id: entry.item_id.clone(),
                        task_key: entry.task_key.clone(),
                        head: entry.head(name)?.to_string(),
                        message: merge_message(
                            &entry.task_key,
                            &entry.title,
                            &target.integration_branch,
                        ),
                    })
                })
                .collect::<Result<Vec<_>, WgError>>()?;
            Ok(RepoRebuild {
                repository_name: name.clone(),
                base_sha: bases.get(name).cloned().unwrap_or_default(),
                merges,
            })
        })
        .collect()
}

/// A replayed merge's result: the head merged and the outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replayed {
    pub repository_name: String,
    pub head: String,
    pub outcome: MergeOutcome,
}

/// An item's detail after its merges were replayed, and the replays that conflicted.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayResult {
    pub detail: ItemDetail,
    /// Replays that conflicted; each is recorded as a conflict of the item.
    pub conflicts: Vec<Replayed>,
}

/// Applies one item's replay results to its stored detail: a clean merge records its new merge
/// commit and clears the repository's conflict; a conflict drops the repository's merge.
/// `results` hold one result per repository in the entry's merges, in the entry's order.
pub fn apply_replay(detail: ItemDetail, results: &[Replayed]) -> ReplayResult {
    let detail = results.iter().fold(detail, |detail, result| {
        let name = &result.repository_name;
        if result.outcome.conflict_paths.is_none() {
            let merge = json!({ "head": result.head, "merge_commit": result.outcome.merge_commit });
            ItemDetail {
                merges: object_with(detail.merges, name, merge),
                conflicts: object_without(detail.conflicts, name),
                other: detail.other,
            }
        } else {
            ItemDetail {
                merges: object_without(detail.merges, name),
                ..detail
            }
        }
    });
    let conflicts = results
        .iter()
        .filter(|result| result.outcome.conflict_paths.is_some())
        .cloned()
        .collect();
    ReplayResult { detail, conflicts }
}

/// The reason the stack returns to `integrating` after a rebuild.
pub const REBUILT_REASON: &str = "rebuilt without ejected items";

/// The `stack.rebuilt` event data.
pub fn rebuilt_event(
    bases: &BTreeMap<String, String>,
    tips: &BTreeMap<String, String>,
    replayed: &[String],
    conflicted: &[String],
) -> Value {
    json!({
        "base_shas": bases,
        "tips": tips,
        "replayed": replayed,
        "conflicted": conflicted,
    })
}
