//! Cumulative churn over a task's linked commits: the lines and files the task's distinct
//! non-merge work changed. Cherry-picked and rewritten copies of one patch count once.
//!
//! Ported from `_cumulative_churn` in `integrations/git/service.py`. The shell loads the rows of
//! the linked commits; this decides the totals.

use std::collections::{BTreeMap, BTreeSet};

use crate::file_class;

/// Structural analysis totals of one commit (`CommitAnalysis`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChurnAnalysis {
    pub functions_affected: u64,
    pub types_affected: u64,
}

/// One file row of a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChurnFile {
    pub path: String,
    pub insertions: u64,
    pub deletions: u64,
}

/// One linked commit as the churn reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChurnCommit {
    pub id: String,
    pub repository_id: String,
    pub patch_id: Option<String>,
    pub is_merge: bool,
    /// Committer date as Unix seconds. `None` sorts after every dated commit.
    pub committed_at: Option<i64>,
    pub insertions: u64,
    pub deletions: u64,
    pub analysis: Option<ChurnAnalysis>,
    pub files: Vec<ChurnFile>,
}

/// Insertions, deletions and file rows of one file class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClassTotals {
    pub insertions: u64,
    pub deletions: u64,
    pub files: u64,
}

/// The churn of a task. `structurally_analyzed_commits` and `by_file_class` are absent when the
/// task has no linked commits at all (the empty form the original returns).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Churn {
    pub commits: u64,
    pub insertions: u64,
    pub deletions: u64,
    pub files_touched: u64,
    pub repeated_file_edits: u64,
    pub functions_affected: u64,
    pub types_affected: u64,
    pub structurally_analyzed_commits: Option<u64>,
    pub excluded_duplicates: u64,
    pub excluded_merges: u64,
    pub by_file_class: Option<BTreeMap<String, ClassTotals>>,
}

/// Cumulative churn of the linked commits. `commits` holds the rows of every linked commit.
pub fn cumulative_churn(commits: &[ChurnCommit]) -> Churn {
    if commits.is_empty() {
        return Churn {
            commits: 0,
            insertions: 0,
            deletions: 0,
            files_touched: 0,
            repeated_file_edits: 0,
            functions_affected: 0,
            types_affected: 0,
            structurally_analyzed_commits: None,
            excluded_duplicates: 0,
            excluded_merges: 0,
            by_file_class: None,
        };
    }

    // Stable sort: ties keep the order the shell passed them in, and a commit without a date
    // comes last, as the original's "now" stand-in did.
    let mut ordered: Vec<&ChurnCommit> = commits.iter().collect();
    ordered.sort_by(|a, b| match (a.committed_at, b.committed_at) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    // Keyed by (repository, patch id): identical patches in different repositories are distinct.
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut counted: Vec<&ChurnCommit> = Vec::new();
    let mut duplicates = 0;
    let mut merges = 0;
    for commit in ordered {
        if commit.is_merge {
            merges += 1;
            continue;
        }
        if let Some(patch) = commit.patch_id.as_deref().filter(|p| !p.is_empty())
            && !seen.insert((commit.repository_id.as_str(), patch))
        {
            duplicates += 1;
            continue;
        }
        counted.push(commit);
    }

    let mut path_rows: BTreeMap<&str, u64> = BTreeMap::new();
    let mut by_class: BTreeMap<String, ClassTotals> = BTreeMap::new();
    for commit in &counted {
        for file in &commit.files {
            *path_rows.entry(file.path.as_str()).or_insert(0) += 1;
            let bucket = by_class
                .entry(file_class(&file.path).to_owned())
                .or_default();
            bucket.insertions += file.insertions;
            bucket.deletions += file.deletions;
            bucket.files += 1;
        }
    }

    let analyzed: Vec<ChurnAnalysis> = counted.iter().filter_map(|c| c.analysis).collect();
    Churn {
        commits: counted.len() as u64,
        insertions: counted.iter().map(|c| c.insertions).sum(),
        deletions: counted.iter().map(|c| c.deletions).sum(),
        files_touched: path_rows.len() as u64,
        repeated_file_edits: path_rows.values().filter(|n| **n > 1).count() as u64,
        functions_affected: analyzed.iter().map(|a| a.functions_affected).sum(),
        types_affected: analyzed.iter().map(|a| a.types_affected).sum(),
        structurally_analyzed_commits: Some(analyzed.len() as u64),
        excluded_duplicates: duplicates,
        excluded_merges: merges,
        by_file_class: Some(by_class),
    }
}
