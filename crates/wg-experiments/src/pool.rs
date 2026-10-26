//! Replay pool decisions: the pure part of `src/workgraph/experiments/pool.py` (CORE-26).
//!
//! The shell lists the workspace's replay tasks, resolves the repository's checkout, runs the
//! commit analysis and creates the tasks. This module decides, for one commit, whether it is
//! skipped, already present or to be created, and shapes the task's title, description and
//! metadata. `golden/expmisc/pool.json` (from `tools/golden/gen_expmisc.py`, which runs the Python
//! pool with its git and database calls stubbed) pins the index, the step and the shaping.

use crate::briefs::shlex_join;
use crate::replay::{CLASSIFIER, Features};
use rpds::RedBlackTreeMapSync;
use serde_json::{Value, json};

/// Tag on every replay task (`REPLAY_TAG`).
pub const REPLAY_TAG: &str = "replay";
/// Tag on every task the pool creates (`POOL_TAG`).
pub const POOL_TAG: &str = "experiment-pool";
/// Longest task title, in characters (`TITLE_MAX`).
pub const TITLE_MAX: usize = 500;

/// The commit-to-task index of the replay tasks already in the workspace. Built with
/// [`existing_index`].
pub type ExistingIndex = RedBlackTreeMapSync<String, String>;

/// A replay task as the index needs it: its key, the commit it replays, and the classifier that
/// built it. The shell supplies the classifier stored in the task's metadata, or the default
/// `replay-classifier-1` when the metadata has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExistingReplay<'a> {
    /// Task key.
    pub key: &'a str,
    /// The replayed commit, when the metadata names one.
    pub sha: Option<&'a str>,
    /// The classifier that built the task.
    pub classifier: &'a str,
}

/// Maps each commit to the first task that replays it, counting only tasks built by the current
/// classifier: a task built under another classifier has different tests and size, so it is not a
/// duplicate.
#[must_use]
pub fn existing_index(tasks: &[ExistingReplay<'_>]) -> ExistingIndex {
    tasks
        .iter()
        .filter(|task| task.classifier == CLASSIFIER)
        .filter_map(|task| task.sha.map(|sha| (sha, task.key)))
        .fold(ExistingIndex::new_sync(), |index, (sha, key)| {
            if index.contains_key(sha) {
                index
            } else {
                index.insert(sha.to_owned(), key.to_owned())
            }
        })
}

/// What the pool does with one commit before any validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The commit is excluded; `reason` joins the exclusion reasons with commas.
    Skipped {
        /// The exclusion reasons, comma-joined.
        reason: String,
    },
    /// A replay task for the commit already exists.
    Exists {
        /// The existing task's key.
        task_key: String,
    },
    /// A new replay task is to be created.
    Create,
}

/// Decides one commit's step: an excluded commit is skipped, a replayed commit is reported with
/// its existing task, and anything else is created.
#[must_use]
pub fn step(excluded: &[&str], sha: &str, index: &ExistingIndex) -> Step {
    if !excluded.is_empty() {
        return Step::Skipped {
            reason: excluded.join(","),
        };
    }
    index
        .get(sha)
        .map_or(Step::Create, |task_key| Step::Exists {
            task_key: task_key.clone(),
        })
}

/// `Replay <sha[:12]>: <subject>`, cut to [`TITLE_MAX`] characters.
#[must_use]
pub fn task_title(sha: &str, subject: &str) -> String {
    let short: String = sha.chars().take(12).collect();
    format!("Replay {short}: {subject}")
        .chars()
        .take(TITLE_MAX)
        .collect()
}

/// The task description: the commit, its message, the tests, the oracle and the base branch.
#[must_use]
pub fn task_description(
    sha: &str,
    subject: &str,
    body: &str,
    test_paths: &[String],
    oracle: &[Vec<String>],
    base_ref: &str,
) -> String {
    let tests = test_paths.iter().map(|path| format!("- `{path}`"));
    let commands = oracle
        .iter()
        .map(|command| format!("- `{}`", shlex_join(command)));
    [
        format!("Replay of commit `{sha}`."),
        String::new(),
        subject.to_owned(),
        String::new(),
        body.to_owned(),
        String::new(),
        "## Tests".to_owned(),
    ]
    .into_iter()
    .chain(tests)
    .chain([String::new(), "## Oracle".to_owned()])
    .chain(commands)
    .chain([String::new(), format!("Base branch: `{base_ref}`")])
    .collect::<Vec<String>>()
    .join("\n")
}

/// The replay metadata of a created task.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry<'a> {
    /// The replayed commit.
    pub sha: &'a str,
    /// Its first parent.
    pub parent: Option<&'a str>,
    /// The branch the replay base was created on.
    pub base_ref: &'a str,
    /// Test files the commit changed.
    pub test_paths: &'a [String],
    /// Source files the commit changed.
    pub source_paths: &'a [String],
    /// The oracle commands.
    pub oracle: &'a [Vec<String>],
    /// The commit's size and shape.
    pub features: &'a Features,
    /// The base and solution exit codes, when the commit was validated.
    pub validation: Option<(&'a [i64], &'a [i64])>,
    /// The validation time, as an ISO-8601 string, when validated.
    pub validated_at: Option<&'a str>,
}

/// The task's metadata object: `{"replay": {...}}`, with the keys the Python source writes.
#[must_use]
pub fn replay_metadata(entry: &Entry<'_>) -> Value {
    let features = entry.features;
    json!({
        "replay": {
            "sha": entry.sha,
            "classifier": CLASSIFIER,
            "parent": entry.parent,
            "base_ref": entry.base_ref,
            "test_paths": entry.test_paths,
            "source_paths": entry.source_paths,
            "oracle": entry.oracle,
            "features": {
                "src_lines": features.src_lines,
                "test_lines": features.test_lines,
                "src_files": features.src_files,
                "test_files": features.test_files,
                "packages": features.packages,
                "band": features.band,
            },
            "validated": entry.validation.is_some(),
            "validation": entry.validation.map(|(base, solution)| json!({
                "base": base,
                "solution": solution,
            })),
            "validated_at": entry.validated_at,
        }
    })
}
