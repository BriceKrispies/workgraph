//! The commit-side fact tables of `analytics/warehouse.py` `_build`: `task_commits` (the commits a
//! task is linked to, de-duplicated across rewrites and cherry-picks by patch id), `task_code`
//! (the churn and the analysis totals of those commits, per task) and `task_files` (the file
//! edits of those commits, per task, module, file class and repository).

use rpds::RedBlackTreeSetSync;
use wg_types::Timestamp;

use crate::agg::{Groups, aggregate, lookup, max_f64, or_zero, sum_opt};
use crate::source::{Analysis, AnalyticsInput, CommitRow, FileChange};

/// A row of `task_commits`: one commit linked to one task, with the highest confidence of its
/// links at or above the threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskCommit {
    pub task_id: Option<String>,
    pub commit: CommitRow,
    pub confidence: Option<f64>,
}

/// Links below this confidence do not count (`l.confidence >= 0.5`).
const MIN_CONFIDENCE: f64 = 0.5;

/// The partition of `ROW_NUMBER()` in `task_commits`: task, repository, and patch id or commit id.
type RankKey = (Option<String>, Option<String>, Option<String>);

/// The group of `task_files`: task, path, module, file class and repository.
type FileKey = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// `task_commits`. Each (task, commit) pair appears once, with its highest link confidence. Among
/// the pairs that share a task, repository and patch id (or the commit id when there is no patch
/// id), the one with the earliest commit time is kept; a NULL time sorts last.
pub fn task_commits(input: &AnalyticsInput) -> Vec<TaskCommit> {
    let commits: Groups<Option<String>, CommitRow> = aggregate(
        input.commits.iter().cloned(),
        |commit| commit.id.clone(),
        |commit| commit.clone(),
        |first, _| first,
    );
    let linked: Groups<(Option<String>, Option<String>), TaskCommit> = aggregate(
        input
            .links
            .iter()
            .filter(|link| link.confidence.is_some_and(|c| c >= MIN_CONFIDENCE))
            .filter_map(|link| lookup(&commits, &link.commit_id).map(|commit| (link, commit))),
        |(link, commit)| (link.task_id.clone(), commit.id.clone()),
        |(link, commit)| TaskCommit {
            task_id: link.task_id.clone(),
            commit: (*commit).clone(),
            confidence: link.confidence,
        },
        |acc, next| TaskCommit {
            confidence: max_f64(acc.confidence, next.confidence),
            ..acc
        },
    );
    let ranked: Groups<RankKey, TaskCommit> = aggregate(
        linked.values().cloned(),
        |row| {
            (
                row.task_id.clone(),
                row.commit.repository_id.clone(),
                row.commit
                    .patch_id
                    .clone()
                    .or_else(|| row.commit.id.clone()),
            )
        },
        |row| row.clone(),
        |acc, next| {
            if committed_order(&next) < committed_order(&acc) {
                next
            } else {
                acc
            }
        },
    );
    ranked.values().cloned().collect()
}

/// The order `ROW_NUMBER() OVER (ORDER BY committed_at)` gives: earliest first, NULL last.
fn committed_order(row: &TaskCommit) -> (bool, Option<Timestamp>) {
    (row.commit.committed_at.is_none(), row.commit.committed_at)
}

/// A row of `task_code`: the non-merge commits of a task and the churn of those commits, with the
/// analysis totals joined in. The join is a LEFT JOIN, so a commit without analysis rows counts
/// once and adds no totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCode {
    pub task_id: Option<String>,
    pub commits: i64,
    pub churn_insertions: i64,
    pub churn_deletions: i64,
    pub repositories_touched: i64,
    pub functions_affected: i64,
    pub types_affected: i64,
}

/// The analysis rows of one commit.
#[derive(Debug, Clone, Copy)]
struct Totals {
    rows: i64,
    functions: Option<i64>,
    types: Option<i64>,
}

impl Totals {
    fn of(analysis: &Analysis) -> Self {
        Self {
            rows: 1,
            functions: analysis.functions_affected,
            types: analysis.types_affected,
        }
    }

    fn merge(a: Self, b: Self) -> Self {
        Self {
            rows: a.rows + b.rows,
            functions: sum_opt(a.functions, b.functions),
            types: sum_opt(a.types, b.types),
        }
    }
}

/// The `task_code` aggregate of one task's commit rows.
#[derive(Debug, Clone)]
struct CodeAcc {
    commits: i64,
    insertions: Option<i64>,
    deletions: Option<i64>,
    repositories: RedBlackTreeSetSync<String>,
    functions: Option<i64>,
    types: Option<i64>,
}

impl CodeAcc {
    /// One task-commit row. Each analysis row of its commit is one joined row, so the commit
    /// counts once per analysis row and its churn is repeated for each of them.
    fn of(tc: &TaskCommit, totals: Option<&Totals>) -> Self {
        let rows = totals.map_or(1, |t| t.rows);
        let counted = tc.commit.is_merge == Some(false);
        Self {
            commits: if counted { rows } else { 0 },
            insertions: if counted {
                tc.commit.insertions.map(|n| n * rows)
            } else {
                None
            },
            deletions: if counted {
                tc.commit.deletions.map(|n| n * rows)
            } else {
                None
            },
            repositories: tc
                .commit
                .repository_id
                .iter()
                .fold(RedBlackTreeSetSync::new_sync(), |set, repo| {
                    set.insert(repo.clone())
                }),
            functions: totals.and_then(|t| t.functions),
            types: totals.and_then(|t| t.types),
        }
    }

    fn merge(a: Self, b: Self) -> Self {
        Self {
            commits: a.commits + b.commits,
            insertions: sum_opt(a.insertions, b.insertions),
            deletions: sum_opt(a.deletions, b.deletions),
            repositories: b
                .repositories
                .iter()
                .fold(a.repositories, |set, repo| set.insert(repo.clone())),
            functions: sum_opt(a.functions, b.functions),
            types: sum_opt(a.types, b.types),
        }
    }

    fn finish(&self, task_id: &Option<String>) -> TaskCode {
        TaskCode {
            task_id: task_id.clone(),
            commits: self.commits,
            churn_insertions: or_zero(self.insertions),
            churn_deletions: or_zero(self.deletions),
            repositories_touched: i64::try_from(self.repositories.size()).unwrap_or(i64::MAX),
            functions_affected: or_zero(self.functions),
            types_affected: or_zero(self.types),
        }
    }
}

/// `task_code`: one row per task id that has rows in `task_commits`.
pub fn task_code(input: &AnalyticsInput, commits: &[TaskCommit]) -> Vec<TaskCode> {
    let analysis: Groups<Option<String>, Totals> = aggregate(
        input.analyses.iter().cloned(),
        |analysis| analysis.commit_id.clone(),
        Totals::of,
        Totals::merge,
    );
    let groups: Groups<Option<String>, CodeAcc> = aggregate(
        commits.iter(),
        |tc| tc.task_id.clone(),
        |tc| CodeAcc::of(tc, lookup(&analysis, &tc.commit.id)),
        CodeAcc::merge,
    );
    groups
        .iter()
        .map(|(task_id, acc)| acc.finish(task_id))
        .collect()
}

/// A row of `task_files`: the edits of one file by one task in one repository. `edits` counts the
/// commits that touched it; `lines` sums the insertions and deletions (NULL when none is known).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFile {
    pub task_id: Option<String>,
    pub path: Option<String>,
    pub module: Option<String>,
    pub file_class: Option<String>,
    pub repository_id: Option<String>,
    pub edits: i64,
    pub lines: Option<i64>,
}

#[derive(Debug, Clone, Copy)]
struct Edits {
    count: i64,
    lines: Option<i64>,
}

/// The lines one file change adds and removes (NULL when either count is unknown).
fn line_count(change: &FileChange) -> Option<i64> {
    change.insertions.zip(change.deletions).map(|(a, b)| a + b)
}

/// `task_files`: the file changes of the non-merge commits in `task_commits`, grouped per task,
/// path, module, file class and repository.
pub fn task_files(input: &AnalyticsInput, commits: &[TaskCommit]) -> Vec<TaskFile> {
    let files: Groups<Option<String>, Vec<FileChange>> = aggregate(
        input.file_changes.iter().cloned(),
        |change| change.commit_id.clone(),
        |change| vec![change.clone()],
        |a, b| a.into_iter().chain(b).collect(),
    );
    let edits: Groups<FileKey, Edits> = aggregate(
        commits
            .iter()
            .filter(|tc| tc.commit.is_merge == Some(false))
            .flat_map(|tc| {
                lookup(&files, &tc.commit.id)
                    .into_iter()
                    .flatten()
                    .map(move |change| (tc, change))
            }),
        |(tc, change)| {
            (
                tc.task_id.clone(),
                change.path.clone(),
                change.module.clone(),
                change.file_class.clone(),
                tc.commit.repository_id.clone(),
            )
        },
        |(_, change)| Edits {
            count: 1,
            lines: line_count(change),
        },
        |a, b| Edits {
            count: a.count + b.count,
            lines: sum_opt(a.lines, b.lines),
        },
    );
    edits
        .iter()
        .map(
            |((task_id, path, module, file_class, repository_id), e)| TaskFile {
                task_id: task_id.clone(),
                path: path.clone(),
                module: module.clone(),
                file_class: file_class.clone(),
                repository_id: repository_id.clone(),
                edits: e.count,
                lines: e.lines,
            },
        )
        .collect()
}
