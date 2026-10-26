//! The pure half of `git sync` (IF-12): the rows of ingested commits, branch membership and
//! reachability updates, and the `SyncResult` shape. The shell runs the git calls and hands their
//! parsed output in; nothing here starts a process or reads the store.
//!
//! Ported from `GitService._ingest_batch`, `_update_branch_membership` and
//! `_reconcile_reachability` in `integrations/git/service.py`.

use std::collections::{BTreeMap, BTreeSet};

use rpds::VectorSync;
use wg_gitcore::{CommitDiff, CommitFacts, CommitMeta, file_class, language_for, module_for};
use wg_json::Value;
use wg_model::entity::JsonObject;
use wg_model::entity::git::{Commit, CommitFileChange};
use wg_model::event::{GitCommitIngested, GitSyncCompleted};
use wg_model::{IdSeq, Row};
use wg_types::{Id, Timestamp};
use wg_wire::convention::{int, model, text};

/// The commit-count limit of the per-commit `git.commit_ingested` events.
pub const PER_COMMIT_EVENT_LIMIT: usize = 200;

/// Python's `META_FORMAT` cut of `subject` and `message`.
const SUBJECT_LIMIT: usize = 1000;
const MESSAGE_LIMIT: usize = 100_000;

/// One ingested commit: its row and the rows of the files it touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestedCommit {
    pub commit: Commit,
    pub files: Vec<CommitFileChange>,
}

/// The rows of one commit read from git, with the ids drawn from `ids` in order: the commit,
/// then one per file. `patch_id` is the stable patch id of a non-merge commit, when git gave one.
pub fn ingested_commit(
    ids: IdSeq,
    repository_id: Id,
    meta: &CommitMeta,
    diff: Option<&CommitDiff>,
    patch_id: Option<&str>,
    now: Timestamp,
) -> (IdSeq, IngestedCommit) {
    let (commit_id, after_commit) = ids.next();
    let files: &[wg_gitcore::FileChange] = diff.map_or(&[], |d| d.files.as_slice());
    let insertions: i64 = files.iter().map(|f| to_i64(f.insertions)).sum();
    let deletions: i64 = files.iter().map(|f| to_i64(f.deletions)).sum();
    let commit = Commit {
        id: commit_id,
        repository_id,
        sha: meta.sha.clone(),
        tree_sha: Some(meta.tree.clone()),
        parents: meta.parents.iter().cloned().collect::<VectorSync<String>>(),
        author_name: meta.author_name.clone(),
        author_email: meta.author_email.clone(),
        authored_at: meta.authored_at,
        committer_name: meta.committer_name.clone(),
        committer_email: meta.committer_email.clone(),
        committed_at: meta.committed_at,
        subject: truncate_chars(meta.subject(), SUBJECT_LIMIT),
        message: truncate_chars(&meta.message, MESSAGE_LIMIT),
        trailers: meta
            .trailers
            .iter()
            .map(trailer_object)
            .collect::<VectorSync<JsonObject>>(),
        is_merge: meta.parents.len() > 1,
        patch_id: patch_id.map(str::to_owned),
        insertions,
        deletions,
        files_changed: to_i64(files.len() as u64),
        reachable: true,
        unreachable_since: None,
        object_available: true,
        structure_analyzed: false,
        branches: VectorSync::new_sync(),
        ingested_at: now,
    };
    let (advanced, rows) = files.iter().fold(
        (after_commit, Vec::with_capacity(files.len())),
        |(seq, mut rows), file| {
            let (file_id, next) = seq.next();
            rows.push(file_row(file_id, commit_id, file));
            (next, rows)
        },
    );
    (
        advanced,
        IngestedCommit {
            commit,
            files: rows,
        },
    )
}

/// One `commit_file_changes` row.
fn file_row(id: Id, commit_id: Id, file: &wg_gitcore::FileChange) -> CommitFileChange {
    CommitFileChange {
        id,
        commit_id,
        path: file.path.clone(),
        old_path: file.old_path.clone(),
        change_type: file.status.clone(),
        similarity: file.similarity.and_then(|value| i64::try_from(value).ok()),
        insertions: to_i64(file.insertions),
        deletions: to_i64(file.deletions),
        is_binary: file.binary,
        language: language_for(&file.path).map(str::to_owned),
        file_class: file_class(&file.path).to_owned(),
        module: module_for(&file.path, 2),
    }
}

/// A trailer as the `trailers` JSON column stores it: `{"key": .., "value": ..}`.
fn trailer_object(trailer: &wg_gitcore::Trailer) -> JsonObject {
    let mut object = JsonObject::new();
    object.insert(
        "key".to_owned(),
        serde_json::Value::String(trailer.key.clone()),
    );
    object.insert(
        "value".to_owned(),
        serde_json::Value::String(trailer.value.clone()),
    );
    object
}

/// Keeps the first `limit` characters, as Python's `text[:limit]` does.
fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// A count as the `i64` the columns hold; counts past `i64::MAX` cannot come from git.
fn to_i64(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// The branch list of each commit after a sync: the stored branches merged with the branches
/// that contain it (sorted, without duplicates). Commits that no membership names are unchanged.
pub fn with_branches(
    commit: &Commit,
    membership: &BTreeMap<String, Vec<String>>,
) -> Option<Commit> {
    let found = membership.get(&commit.sha)?;
    let merged: BTreeSet<String> = commit
        .branches
        .iter()
        .cloned()
        .chain(found.iter().cloned())
        .collect();
    Some(Commit {
        branches: merged.into_iter().collect::<VectorSync<String>>(),
        ..commit.clone()
    })
}

/// The reachability update of one stored commit, when its state changes: a reachable commit that
/// left every ref becomes unreachable (with the time and whether its object still exists), and an
/// unreachable one that a ref reaches again is restored.
pub fn reachability_change(
    commit: &Commit,
    reachable_now: &BTreeSet<String>,
    object_exists: impl Fn(&str) -> bool,
    now: Timestamp,
) -> Option<Commit> {
    let reachable = reachable_now.contains(&commit.sha);
    match (commit.reachable, reachable) {
        (true, false) => Some(Commit {
            reachable: false,
            unreachable_since: Some(now),
            object_available: object_exists(&commit.sha),
            ..commit.clone()
        }),
        (false, true) => Some(Commit {
            reachable: true,
            unreachable_since: None,
            object_available: true,
            ..commit.clone()
        }),
        _ => None,
    }
}

/// The `SyncResult` the sync reports (`repository_id`, `repository`, `mode`, then the counts).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncResult {
    pub repository_id: String,
    pub repository: String,
    pub mode: String,
    pub new_commits: i64,
    pub refs_changed: i64,
    pub marked_unreachable: i64,
    pub restored_reachable: i64,
    pub equivalences: i64,
    pub links_created: i64,
    pub shallow: bool,
    pub head_refs: i64,
}

impl SyncResult {
    /// The `git.sync_completed` payload of this result.
    pub fn completed_payload(&self, repository_id: Id) -> GitSyncCompleted {
        GitSyncCompleted {
            equivalences: serde_json::Value::from(self.equivalences),
            head_refs: serde_json::Value::from(self.head_refs),
            links_created: self.links_created,
            marked_unreachable: self.marked_unreachable,
            mode: self.mode.clone(),
            new_commits: self.new_commits,
            refs_changed: serde_json::Value::from(self.refs_changed),
            repository: self.repository.clone(),
            repository_id,
            restored_reachable: self.restored_reachable,
            shallow: self.shallow,
        }
    }
}

/// The `git.commit_ingested` payload of one new commit.
pub fn commit_ingested_payload(
    commit: &Commit,
    repository_id: Id,
    task_ids: &[Id],
) -> GitCommitIngested {
    GitCommitIngested {
        deletions: commit.deletions,
        insertions: commit.insertions,
        repository_id,
        sha: commit.sha.clone(),
        subject: truncate_chars(&commit.subject, 200),
        task_ids: task_ids.iter().copied().collect(),
    }
}

/// The rows of `ingested` as writes, in the order the sync inserts them (commit, then its files).
pub fn insert_rows(ingested: &[IngestedCommit]) -> Vec<Row> {
    ingested
        .iter()
        .flat_map(|item| {
            std::iter::once(Row::Commit(item.commit.clone())).chain(
                item.files
                    .iter()
                    .map(|file| Row::CommitFileChange(file.clone())),
            )
        })
        .collect()
}

/// The `new_shas` of a sync: the candidates the store does not hold yet, oldest first. `candidates`
/// is `rev-list --all` order (newest first).
pub fn new_shas(candidates: &[String], known: &BTreeSet<String>) -> Vec<String> {
    candidates
        .iter()
        .rev()
        .filter(|sha| !known.contains(*sha))
        .cloned()
        .collect()
}

/// Whether a sync must reconcile reachability: a full sync always does, an incremental one when a
/// ref was removed or moved to a commit that is not a descendant of its old value.
pub fn needs_reconcile(full: bool, non_fast_forward: bool) -> bool {
    full || non_fast_forward
}

/// The `diff-tree` input line of a commit: its first parent, when it has one.
pub fn diff_line(meta: &CommitMeta) -> String {
    match meta.parents.first() {
        Some(parent) => format!("{} {parent}", meta.sha),
        None => meta.sha.clone(),
    }
}

/// `now` moved forward by `micros` microseconds. Rows written by one decision are stamped this way,
/// so each keeps its own instant and the store's `created_at` order is the order they were written
/// in, as it is when each row reads the clock itself.
pub fn offset_time(now: Timestamp, micros: usize) -> Timestamp {
    let shifted = now.to_offset_datetime()
        + time::Duration::microseconds(i64::try_from(micros).unwrap_or(i64::MAX));
    Timestamp::from_offset_datetime(shifted).unwrap_or(now)
}

/// A count of rows as the `i64` the results hold.
pub fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The facts of a stored commit that the equivalence decision reads.
pub fn equivalence_facts(commit: &Commit) -> CommitFacts {
    CommitFacts {
        id: commit.id.to_string(),
        sha: commit.sha.clone(),
        repository_id: commit.repository_id.to_string(),
        patch_id: commit.patch_id.clone(),
        reachable: Some(commit.reachable),
        author_email: commit.author_email.clone(),
        authored_at: commit
            .authored_at
            .map(|at| at.to_offset_datetime().unix_timestamp()),
        subject: commit.subject.clone(),
        message: commit.message.clone(),
    }
}

/// The `SyncResult` model value, fields in the Python class's order.
pub fn sync_result_value(result: &SyncResult) -> Value {
    model(
        "SyncResult",
        vec![
            ("repository_id", text(&result.repository_id)),
            ("repository", text(&result.repository)),
            ("mode", text(&result.mode)),
            ("new_commits", int(result.new_commits)),
            ("refs_changed", int(result.refs_changed)),
            ("marked_unreachable", int(result.marked_unreachable)),
            ("restored_reachable", int(result.restored_reachable)),
            ("equivalences", int(result.equivalences)),
            ("links_created", int(result.links_created)),
            ("shallow", Value::Bool(result.shallow)),
            ("head_refs", int(result.head_refs)),
        ],
    )
}
