//! The pure half of the git reads (`git log`, `git show`, `git changes`, `git diff`): the JSON each
//! one prints, built from the rows the shell loaded. Ported from `repository_commits`,
//! `commit_detail`, `task_changes` and `task_diff` in `integrations/git/service.py`.

use std::collections::BTreeSet;
use wg_gitcore::{Churn as GitChurn, ChurnAnalysis, ChurnCommit, ChurnFile, ClassTotals};
use wg_json::Value;
use wg_model::entity::git::{
    Commit, CommitAnalysis, CommitEquivalence, CommitFileChange, CommitSymbol, CommitTaskLink,
    TaskChangeSurface,
};

use wg_storecodec::git_sql::TaskCommitLink as TaskCommitLinkRow;
use wg_types::Id;
use wg_wire::convention::{
    TimeFormat, dict, id, int, json_object, optional_text, optional_timestamp, text,
};
use wg_wire::dto::task_changes::{self, Churn as WireChurn, ChurnTotals, CommitEntry, TaskChanges};
use wg_wire::dto::task_diff::{self, DiffPart, Missing, TaskDiff};

/// One task link of a commit, with the task's key, as the commit listings show it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRef {
    pub task_id: Id,
    pub task_key: String,
    pub source: String,
    pub confidence: f64,
}

/// One row of `git log`: a stored commit and its task links.
#[derive(Debug, Clone, PartialEq)]
pub struct LogItem<'a> {
    pub commit: &'a Commit,
    pub tasks: Vec<TaskRef>,
}

/// The `git log` page (`repository_commits`): `repository_id`, `total` and the items.
pub fn repository_commits_value(repository_id: Id, total: i64, items: &[LogItem<'_>]) -> Value {
    dict(vec![
        ("repository_id", id(repository_id)),
        ("total", int(total)),
        ("items", Value::List(items.iter().map(log_item).collect())),
    ])
}

fn log_item(item: &LogItem<'_>) -> Value {
    let commit = item.commit;
    dict(vec![
        ("id", id(commit.id)),
        ("sha", text(&commit.sha)),
        ("subject", text(&commit.subject)),
        ("author", text(&commit.author_name)),
        ("author_email", text(&commit.author_email)),
        (
            "committed_at",
            optional_timestamp(commit.committed_at, TimeFormat::Isoformat),
        ),
        ("insertions", int(commit.insertions)),
        ("deletions", int(commit.deletions)),
        ("files_changed", int(commit.files_changed)),
        ("is_merge", Value::Bool(commit.is_merge)),
        ("reachable", Value::Bool(commit.reachable)),
        (
            "branches",
            Value::List(commit.branches.iter().map(|b| text(b)).collect()),
        ),
        ("trailers", trailers_value(commit)),
        (
            "tasks",
            Value::List(item.tasks.iter().map(task_ref_value).collect()),
        ),
    ])
}

/// The stored trailers, each `{key, value}`, in message order.
fn trailers_value(commit: &Commit) -> Value {
    Value::List(commit.trailers.iter().map(json_object).collect())
}

/// `{task_id, task_key, source, confidence}` of one link of a listing.
fn task_ref_value(task: &TaskRef) -> Value {
    dict(vec![
        ("task_id", id(task.task_id)),
        ("task_key", text(&task.task_key)),
        ("source", text(&task.source)),
        ("confidence", Value::Float(task.confidence)),
    ])
}

/// The inputs of `git show`: the commit, its file rows with their symbols, the analysis, the
/// equivalences in either direction, and the task links with their keys.
#[derive(Debug, Clone, Copy)]
pub struct DetailInput<'a> {
    pub commit: &'a Commit,
    pub files: &'a [CommitFileChange],
    pub symbols: &'a [CommitSymbol],
    pub analysis: Option<&'a CommitAnalysis>,
    pub equivalences: &'a [CommitEquivalence],
    pub links: &'a [(CommitTaskLink, String)],
}

/// The `git show` dict (`commit_detail`).
pub fn commit_detail_value(input: &DetailInput<'_>) -> Value {
    let commit = input.commit;
    dict(vec![
        ("id", id(commit.id)),
        ("sha", text(&commit.sha)),
        ("repository_id", id(commit.repository_id)),
        (
            "parents",
            Value::List(commit.parents.iter().map(|p| text(p)).collect()),
        ),
        ("subject", text(&commit.subject)),
        ("message", text(&commit.message)),
        ("trailers", trailers_value(commit)),
        (
            "author",
            person(
                &commit.author_name,
                &commit.author_email,
                commit.authored_at,
            ),
        ),
        (
            "committer",
            person(
                &commit.committer_name,
                &commit.committer_email,
                commit.committed_at,
            ),
        ),
        ("is_merge", Value::Bool(commit.is_merge)),
        ("patch_id", optional_text(commit.patch_id.as_deref())),
        ("reachable", Value::Bool(commit.reachable)),
        ("object_available", Value::Bool(commit.object_available)),
        (
            "branches",
            Value::List(commit.branches.iter().map(|b| text(b)).collect()),
        ),
        ("insertions", int(commit.insertions)),
        ("deletions", int(commit.deletions)),
        (
            "files",
            Value::List(
                input
                    .files
                    .iter()
                    .map(|file| file_detail(file, input.symbols))
                    .collect(),
            ),
        ),
        (
            "analysis",
            input.analysis.map_or(Value::Null, analysis_value),
        ),
        (
            "equivalences",
            Value::List(input.equivalences.iter().map(equivalence_value).collect()),
        ),
        (
            "tasks",
            Value::List(
                input
                    .links
                    .iter()
                    .map(|(link, key)| link_value(link, key))
                    .collect(),
            ),
        ),
    ])
}

fn person(name: &str, email: &str, at: Option<wg_types::Timestamp>) -> Value {
    dict(vec![
        ("name", text(name)),
        ("email", text(email)),
        ("at", optional_timestamp(at, TimeFormat::Isoformat)),
    ])
}

fn file_detail(file: &CommitFileChange, symbols: &[CommitSymbol]) -> Value {
    let own: Vec<Value> = symbols
        .iter()
        .filter(|symbol| symbol.file_change_id == file.id)
        .map(|symbol| {
            dict(vec![
                ("kind", text(&symbol.symbol_kind)),
                ("name", text(&symbol.name)),
                ("change", text(&symbol.change_kind)),
                ("lines_changed", int(symbol.lines_changed)),
            ])
        })
        .collect();
    dict(vec![
        ("path", text(&file.path)),
        ("old_path", optional_text(file.old_path.as_deref())),
        ("change_type", text(&file.change_type)),
        ("insertions", int(file.insertions)),
        ("deletions", int(file.deletions)),
        ("binary", Value::Bool(file.is_binary)),
        ("language", optional_text(file.language.as_deref())),
        ("file_class", text(&file.file_class)),
        ("module", text(&file.module)),
        ("symbols", Value::List(own)),
    ])
}

fn analysis_value(analysis: &CommitAnalysis) -> Value {
    dict(vec![
        ("analyzer_version", text(&analysis.analyzer_version)),
        ("functions_affected", int(analysis.functions_affected)),
        ("types_affected", int(analysis.types_affected)),
        ("files_parsed", int(analysis.files_parsed)),
        ("files_fallback", int(analysis.files_fallback)),
        ("detail", json_object(&analysis.detail)),
    ])
}

fn equivalence_value(equivalence: &CommitEquivalence) -> Value {
    dict(vec![
        ("kind", text(&equivalence.kind)),
        ("original_commit_id", id(equivalence.original_commit_id)),
        ("equivalent_commit_id", id(equivalence.equivalent_commit_id)),
        ("evidence", json_object(&equivalence.evidence)),
    ])
}

fn link_value(link: &CommitTaskLink, key: &str) -> Value {
    dict(vec![
        ("task_id", id(link.task_id)),
        ("task_key", text(key)),
        ("source", text(&link.source)),
        ("confidence", Value::Float(link.confidence.0)),
        ("evidence", json_object(&link.evidence)),
    ])
}

/// One linked commit of `git changes`, with the repository name and the links that attribute it.
#[derive(Debug, Clone, Copy)]
pub struct ChangeCommit<'a> {
    pub commit: &'a Commit,
    pub repository_name: &'a str,
    pub links: &'a [&'a CommitTaskLink],
}

/// The inputs of `git changes`: the linked commits (in the service's order), the analysis and
/// the file rows the churn reads, and the task's change surfaces.
pub struct ChangesInput<'a> {
    pub task_id: Id,
    pub task_key: &'a str,
    pub min_confidence: f64,
    pub commits: &'a [ChangeCommit<'a>],
    pub files: &'a [CommitFileChange],
    pub analyses: &'a [CommitAnalysis],
    pub surfaces: &'a [&'a TaskChangeSurface],
}

/// The `git changes` dict (`task_changes`): the commits, the cumulative churn and the surfaces.
pub fn task_changes_value(input: &ChangesInput<'_>) -> Value {
    let entries: Vec<CommitEntry<'_>> = input
        .commits
        .iter()
        .map(|item| CommitEntry {
            commit: item.commit,
            repository_name: item.repository_name,
            links: item.links,
        })
        .collect();
    let churn = wire_churn(&cumulative(input));
    task_changes::from_parts(&TaskChanges {
        task_id: input.task_id,
        task_key: input.task_key,
        min_confidence: input.min_confidence,
        commits: &entries,
        churn: &churn,
        surfaces: input.surfaces,
    })
}

/// The churn of the commits of `input`, computed by the pure churn decision.
fn cumulative(input: &ChangesInput<'_>) -> GitChurn {
    let churn_commits: Vec<ChurnCommit> = input
        .commits
        .iter()
        .map(|item| churn_commit(item.commit, input))
        .collect();
    wg_gitcore::cumulative_churn(&churn_commits)
}

fn churn_commit(commit: &Commit, input: &ChangesInput<'_>) -> ChurnCommit {
    let analysis = input
        .analyses
        .iter()
        .find(|analysis| analysis.commit_id == commit.id)
        .map(|analysis| ChurnAnalysis {
            functions_affected: count(analysis.functions_affected),
            types_affected: count(analysis.types_affected),
        });
    let files = input
        .files
        .iter()
        .filter(|file| file.commit_id == commit.id)
        .map(|file| ChurnFile {
            path: file.path.clone(),
            insertions: count(file.insertions),
            deletions: count(file.deletions),
        })
        .collect();
    ChurnCommit {
        id: commit.id.to_string(),
        repository_id: commit.repository_id.to_string(),
        patch_id: commit.patch_id.clone(),
        is_merge: commit.is_merge,
        committed_at: commit
            .committed_at
            .map(|at| at.to_offset_datetime().unix_timestamp()),
        insertions: count(commit.insertions),
        deletions: count(commit.deletions),
        analysis,
        files,
    }
}

/// A stored count as the unsigned count the churn takes; negative values cannot be stored.
fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// The DTO form of a churn: `NoCommits` when there were no linked commits at all.
fn wire_churn(churn: &GitChurn) -> WireChurn {
    match churn.by_file_class.as_ref() {
        None if churn.commits == 0 && churn.structurally_analyzed_commits.is_none() => {
            WireChurn::NoCommits
        }
        by_class => WireChurn::Counted(ChurnTotals {
            commits: signed(churn.commits),
            insertions: signed(churn.insertions),
            deletions: signed(churn.deletions),
            files_touched: signed(churn.files_touched),
            repeated_file_edits: signed(churn.repeated_file_edits),
            functions_affected: signed(churn.functions_affected),
            types_affected: signed(churn.types_affected),
            structurally_analyzed_commits: signed(churn.structurally_analyzed_commits.unwrap_or(0)),
            excluded_duplicates: signed(churn.excluded_duplicates),
            excluded_merges: signed(churn.excluded_merges),
            by_file_class: by_class.map_or_else(Vec::new, |classes| {
                classes
                    .iter()
                    .map(|(class, totals)| (class.clone(), class_totals(totals)))
                    .collect()
            }),
        }),
    }
}

fn class_totals(totals: &ClassTotals) -> task_changes::ClassTotals {
    task_changes::ClassTotals {
        insertions: signed(totals.insertions),
        deletions: signed(totals.deletions),
        files: signed(totals.files),
    }
}

fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The `git diff` dict (`task_diff`), with the parts cut to `max_bytes` by the DTO.
pub fn task_diff_value(
    task_id: Id,
    task_key: &str,
    parts: &[DiffPart],
    missing: &[Missing],
    max_bytes: usize,
) -> Value {
    task_diff::from_parts(&TaskDiff {
        task_id,
        task_key,
        parts,
        missing,
        max_bytes,
    })
}

/// The ids of a slice of commits, in order.
pub fn commit_ids(commits: &[Commit]) -> Vec<Id> {
    commits.iter().map(|commit| commit.id).collect()
}

/// The distinct ids of a sequence, in first-seen order.
pub fn unique_commit_ids(ids: impl Iterator<Item = Id>) -> Vec<Id> {
    let mut seen = BTreeSet::new();
    ids.filter(|id| seen.insert(*id)).collect()
}

/// The first twelve characters of a SHA, as the diff ranges print them.
pub fn short_sha(sha: &str) -> String {
    sha.chars().take(12).collect()
}

/// A stored count as the `i64` the rows hold.
pub fn to_i64(value: impl TryInto<i64>) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

/// A count as the `u64` the churn and surface records use.
pub fn to_u64(value: impl TryInto<u64>) -> u64 {
    value.try_into().unwrap_or(0)
}

/// A commit with its links and repository name, in the order the links first name it.
#[derive(Debug, Clone)]
pub struct CommitGroup {
    pub commit: Commit,
    pub repository_name: String,
    pub links: Vec<CommitTaskLink>,
}

/// Groups task-commit rows by commit, keeping the order the rows name each commit in.
pub fn group_by_commit(rows: &[TaskCommitLinkRow]) -> Vec<CommitGroup> {
    let mut groups: Vec<CommitGroup> = Vec::new();
    for row in rows {
        match groups
            .iter_mut()
            .find(|group| group.commit.id == row.commit.id)
        {
            Some(group) => group.links.push(row.link.clone()),
            None => groups.push(CommitGroup {
                commit: row.commit.clone(),
                repository_name: row.repository_name.clone(),
                links: vec![row.link.clone()],
            }),
        }
    }
    groups
}

/// The gap targets of a missing-object list: the repository id and the SHA of each. `repos` is
/// each repository of the workspace with its name.
pub fn missing_gap_targets(missing: &[Missing], repos: &[(Id, String)]) -> Vec<(Id, String)> {
    missing
        .iter()
        .filter_map(|item| {
            let (name, sha) = match item {
                Missing::Commit { repository, sha } => (repository, sha.clone()),
                Missing::Range { repository, range } => (
                    repository,
                    range.rsplit("..").next().unwrap_or_default().to_owned(),
                ),
            };
            let repository_id = repos.iter().find(|(_, repo)| repo == name)?.0;
            (!sha.is_empty()).then_some((repository_id, sha))
        })
        .collect()
}
