//! The output of `GitService.task_changes` (MCP `git_get_changes`, `GET /tasks/{ref}/changes`): a
//! plain dict, not a Pydantic model, so there is no OpenAPI component and it is not listed in
//! [`super::implemented`].
//!
//! The service computes the commit list, the churn and the surfaces; this module only prints
//! them in the service's key order. `committed_at` and the churn are the service's own
//! `isoformat()` output, so timestamps use `TimeFormat::Isoformat` (the MCP recording shows
//! `+00:00`).

use wg_json::{Value, dict};
use wg_model::entity::git::{Commit, CommitTaskLink, TaskChangeSurface};
use wg_types::Id;

use crate::convention::{TimeFormat, id, int, json_object, optional_timestamp, text};

/// The keys of the top-level dict, in the order the service builds them.
pub const FIELDS: [&str; 7] = [
    "task_id",
    "task_key",
    "min_confidence",
    "commits",
    "cumulative_churn",
    "final_change_surface",
    "definitions",
];

/// The keys of one entry of `commits`.
pub const COMMIT_FIELDS: [&str; 15] = [
    "commit_id",
    "sha",
    "repository_id",
    "repository",
    "subject",
    "author",
    "committed_at",
    "insertions",
    "deletions",
    "files_changed",
    "is_merge",
    "reachable",
    "object_available",
    "links",
    "confidence",
];

/// The keys of one entry of `final_change_surface`.
pub const SURFACE_FIELDS: [&str; 11] = [
    "repository_id",
    "base_sha",
    "final_sha",
    "files_changed",
    "insertions",
    "deletions",
    "renames",
    "binary_files",
    "functions_affected",
    "types_affected",
    "detail",
];

/// The `definitions` text of the service, verbatim.
pub const CUMULATIVE_CHURN_DEFINITION: &str = "Sum of lines added/removed across distinct linked non-merge commits; cherry-picked/rewritten duplicates of the same change are counted once.";
/// The `definitions` text of the service, verbatim.
pub const FINAL_SURFACE_DEFINITION: &str =
    "git diff between the attempt baseline and the accepted revision, per repository.";

/// One linked commit with the links that attribute it to the task. `links` must be in the order
/// the service reads them (rows ordered by the commit's `committed_at`).
pub struct CommitEntry<'a> {
    pub commit: &'a Commit,
    pub repository_name: &'a str,
    pub links: &'a [&'a CommitTaskLink],
}

/// The cumulative churn of the counted commits, as the service computes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Churn {
    /// No linked commit at all: the service returns zeros and no per-class breakdown.
    NoCommits,
    /// The counted totals and the excluded counts.
    Counted(ChurnTotals),
}

/// The counted totals of a non-empty commit set.
#[derive(Debug, Clone, PartialEq)]
pub struct ChurnTotals {
    pub commits: i64,
    pub insertions: i64,
    pub deletions: i64,
    pub files_touched: i64,
    pub repeated_file_edits: i64,
    pub functions_affected: i64,
    pub types_affected: i64,
    pub structurally_analyzed_commits: i64,
    pub excluded_duplicates: i64,
    pub excluded_merges: i64,
    /// Per file class, in the order the caller lists them.
    pub by_file_class: Vec<(String, ClassTotals)>,
}

/// The totals of one file class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassTotals {
    pub insertions: i64,
    pub deletions: i64,
    pub files: i64,
}

/// The whole output dict. `commits` and `surfaces` keep the order the caller gives.
pub struct TaskChanges<'a> {
    pub task_id: Id,
    pub task_key: &'a str,
    pub min_confidence: f64,
    pub commits: &'a [CommitEntry<'a>],
    pub churn: &'a Churn,
    pub surfaces: &'a [&'a TaskChangeSurface],
}

/// The task-changes dict.
pub fn from_parts(changes: &TaskChanges<'_>) -> Value {
    dict!({
        "task_id": id(changes.task_id),
        "task_key": text(changes.task_key),
        "min_confidence": Value::Float(changes.min_confidence),
        "commits": Value::List(changes.commits.iter().map(commit_entry).collect()),
        "cumulative_churn": churn(changes.churn),
        "final_change_surface": Value::List(changes.surfaces.iter().copied().map(surface).collect()),
        "definitions": dict!({
            "cumulative_churn": text(CUMULATIVE_CHURN_DEFINITION),
            "final_change_surface": text(FINAL_SURFACE_DEFINITION),
        }),
    })
}

/// One entry of `commits`. `confidence` is the highest link confidence, starting from `0.0`.
pub fn commit_entry(entry: &CommitEntry<'_>) -> Value {
    let commit = entry.commit;
    let confidence = entry
        .links
        .iter()
        .fold(0.0_f64, |best, link| best.max(link.confidence.0));
    dict!({
        "commit_id": id(commit.id),
        "sha": text(&commit.sha),
        "repository_id": id(commit.repository_id),
        "repository": text(entry.repository_name),
        "subject": text(&commit.subject),
        "author": text(&commit.author_name),
        "committed_at": optional_timestamp(commit.committed_at, TimeFormat::Isoformat),
        "insertions": int(commit.insertions),
        "deletions": int(commit.deletions),
        "files_changed": int(commit.files_changed),
        "is_merge": Value::Bool(commit.is_merge),
        "reachable": Value::Bool(commit.reachable),
        "object_available": Value::Bool(commit.object_available),
        "links": Value::List(entry.links.iter().map(|link| link_entry(link)).collect()),
        "confidence": Value::Float(confidence),
    })
}

fn link_entry(link: &CommitTaskLink) -> Value {
    dict!({
        "source": text(&link.source),
        "confidence": Value::Float(link.confidence.0),
        "evidence": json_object(&link.evidence),
        "attempt_id": link.attempt_id.map_or(Value::Null, id),
    })
}

/// The `cumulative_churn` dict: nine keys when no commit is linked, eleven otherwise.
pub fn churn(churn: &Churn) -> Value {
    match churn {
        Churn::NoCommits => dict!({
            "commits": int(0),
            "insertions": int(0),
            "deletions": int(0),
            "files_touched": int(0),
            "repeated_file_edits": int(0),
            "functions_affected": int(0),
            "types_affected": int(0),
            "excluded_duplicates": int(0),
            "excluded_merges": int(0),
        }),
        Churn::Counted(totals) => dict!({
            "commits": int(totals.commits),
            "insertions": int(totals.insertions),
            "deletions": int(totals.deletions),
            "files_touched": int(totals.files_touched),
            "repeated_file_edits": int(totals.repeated_file_edits),
            "functions_affected": int(totals.functions_affected),
            "types_affected": int(totals.types_affected),
            "structurally_analyzed_commits": int(totals.structurally_analyzed_commits),
            "excluded_duplicates": int(totals.excluded_duplicates),
            "excluded_merges": int(totals.excluded_merges),
            "by_file_class": dict(
                totals
                    .by_file_class
                    .iter()
                    .map(|(class, t)| {
                        (
                            class.as_str(),
                            dict!({
                                "insertions": int(t.insertions),
                                "deletions": int(t.deletions),
                                "files": int(t.files),
                            }),
                        )
                    })
                    .collect(),
            ),
        }),
    }
}

/// One entry of `final_change_surface`.
pub fn surface(surface: &TaskChangeSurface) -> Value {
    dict!({
        "repository_id": id(surface.repository_id),
        "base_sha": text(&surface.base_sha),
        "final_sha": text(&surface.final_sha),
        "files_changed": int(surface.files_changed),
        "insertions": int(surface.insertions),
        "deletions": int(surface.deletions),
        "renames": int(surface.renames),
        "binary_files": int(surface.binary_files),
        "functions_affected": int(surface.functions_affected),
        "types_affected": int(surface.types_affected),
        "detail": json_object(&surface.detail),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_churn_has_nine_keys() {
        let Value::Dict(fields) = churn(&Churn::NoCommits) else {
            panic!("churn is a dict");
        };
        assert_eq!(fields.len(), 9);
    }
}
