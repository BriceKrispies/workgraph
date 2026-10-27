//! The output of `GitService.task_changes` (MCP `git_get_changes`, `GET /tasks/{ref}/changes`): a
//! plain dict, not a Pydantic model, so there is no OpenAPI component and it is not listed in
//! [`super::implemented`].
//!
//! The service computes the commit list, the churn and the surfaces; this module only prints
//! them in the service's key order. `committed_at` and the churn are the service's own
//! `isoformat()` output, so timestamps use the `isoformat` adapter (the MCP recording shows
//! `+00:00`).

use serde::{Serialize, Serializer};
use serde_json::{Map, Value as Json};
use wg_model::entity::git::{Commit, CommitTaskLink, TaskChangeSurface};
use wg_types::{Id, Timestamp};

use crate::convention::{isoformat, json_map};
use crate::wire_dto;

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

/// The counted totals of a non-empty commit set; the fields are the keys of `cumulative_churn`.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    #[serde(serialize_with = "by_class")]
    pub by_file_class: Vec<(String, ClassTotals)>,
}

/// The totals of one file class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClassTotals {
    pub insertions: i64,
    pub deletions: i64,
    pub files: i64,
}

fn by_class<S: Serializer>(
    classes: &[(String, ClassTotals)],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_map(classes.iter().map(|(class, totals)| (class, totals)))
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

/// The `cumulative_churn` dict when no commit is linked: nine keys, all zero.
#[derive(Debug, Serialize)]
pub struct NoCommitsChurn {
    pub commits: i64,
    pub insertions: i64,
    pub deletions: i64,
    pub files_touched: i64,
    pub repeated_file_edits: i64,
    pub functions_affected: i64,
    pub types_affected: i64,
    pub excluded_duplicates: i64,
    pub excluded_merges: i64,
}

/// The `cumulative_churn` value: the zero dict or the counted totals.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ChurnOut<'a> {
    None(NoCommitsChurn),
    Counted(&'a ChurnTotals),
}

/// The `definitions` block.
#[derive(Debug, Serialize)]
pub struct Definitions {
    pub cumulative_churn: &'static str,
    pub final_change_surface: &'static str,
}

wire_dto! {
    /// The task-changes dict, keys in the order the service builds them.
    pub struct TaskChangesOut<'a>("TaskChangesOut") dict from_parts(c: &'a TaskChanges<'a>) {
        task_id: Id = c.task_id,
        task_key: &'a str = c.task_key,
        min_confidence: f64 = c.min_confidence,
        commits: Vec<CommitOut<'a>> = c.commits.iter().map(CommitOut::new).collect(),
        cumulative_churn: ChurnOut<'a> = churn_out(c.churn),
        final_change_surface: Vec<SurfaceOut<'a>> = c.surfaces.iter().copied().map(SurfaceOut::new).collect(),
        definitions: Definitions = Definitions {
            cumulative_churn: CUMULATIVE_CHURN_DEFINITION,
            final_change_surface: FINAL_SURFACE_DEFINITION,
        },
    }
}

wire_dto! {
    /// One entry of `commits`. `confidence` is the highest link confidence, starting from `0.0`.
    pub struct CommitOut<'a>("CommitOut") dict commit_entry(e: &'a CommitEntry<'a>) {
        commit_id: Id = e.commit.id,
        sha: &'a str = &e.commit.sha,
        repository_id: Id = e.commit.repository_id,
        repository: &'a str = e.repository_name,
        subject: &'a str = &e.commit.subject,
        author: &'a str = &e.commit.author_name,
        #[serde(serialize_with = "isoformat")]
        committed_at: Option<Timestamp> = e.commit.committed_at,
        insertions: i64 = e.commit.insertions,
        deletions: i64 = e.commit.deletions,
        files_changed: i64 = e.commit.files_changed,
        is_merge: bool = e.commit.is_merge,
        reachable: bool = e.commit.reachable,
        object_available: bool = e.commit.object_available,
        links: Vec<LinkOut<'a>> = e.links.iter().copied().map(LinkOut::new).collect(),
        confidence: f64 = e.links.iter().fold(0.0_f64, |best, link| best.max(link.confidence.0)),
    }
}

wire_dto! {
    /// One link that attributes a commit to the task.
    pub struct LinkOut<'a>("LinkOut") dict link_entry(l: &'a CommitTaskLink) {
        source: &'a str = &l.source,
        confidence: f64 = l.confidence.0,
        #[serde(serialize_with = "json_map")]
        evidence: &'a Map<String, Json> = &l.evidence,
        attempt_id: Option<Id> = l.attempt_id,
    }
}

wire_dto! {
    /// One entry of `final_change_surface`.
    pub struct SurfaceOut<'a>("SurfaceOut") dict surface(s: &'a TaskChangeSurface) {
        repository_id: Id = s.repository_id,
        base_sha: &'a str = &s.base_sha,
        final_sha: &'a str = &s.final_sha,
        files_changed: i64 = s.files_changed,
        insertions: i64 = s.insertions,
        deletions: i64 = s.deletions,
        renames: i64 = s.renames,
        binary_files: i64 = s.binary_files,
        functions_affected: i64 = s.functions_affected,
        types_affected: i64 = s.types_affected,
        #[serde(serialize_with = "json_map")]
        detail: &'a Map<String, Json> = &s.detail,
    }
}

fn churn_out(churn: &Churn) -> ChurnOut<'_> {
    match churn {
        Churn::NoCommits => ChurnOut::None(NoCommitsChurn {
            commits: 0,
            insertions: 0,
            deletions: 0,
            files_touched: 0,
            repeated_file_edits: 0,
            functions_affected: 0,
            types_affected: 0,
            excluded_duplicates: 0,
            excluded_merges: 0,
        }),
        Churn::Counted(totals) => ChurnOut::Counted(totals),
    }
}

/// The `cumulative_churn` dict: nine keys when no commit is linked, eleven otherwise.
pub fn churn(churn: &Churn) -> wg_json::Value {
    wg_json::to_dict(&churn_out(churn))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_json::Value;

    #[test]
    fn empty_churn_has_nine_keys() {
        let Value::Dict(fields) = churn(&Churn::NoCommits) else {
            panic!("churn is a dict");
        };
        assert_eq!(fields.len(), 9);
    }
}
