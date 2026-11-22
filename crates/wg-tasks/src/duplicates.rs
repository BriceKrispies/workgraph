//! The duplicate check of a task create (`tasks.duplicate_check`, `tasks.duplicate_threshold`).
//!
//! Agents create tasks without looking for existing ones, so a create is compared with the open
//! tasks of the same repository or project before it is written. The score is a token overlap over
//! the normalised title and description, computed offline and deterministically: the same two
//! tasks always score the same, with no embedding model involved.
//!
//! The score of two tasks is the mean of the Jaccard overlap of their title tokens and the Jaccard
//! overlap of the tokens of title and description together, so a matching title with an unrelated
//! description scores 0.5 at most and a copy scores 1.0. Tokens are lower-case alphanumeric words
//! without a few stop words and with a plural `s` removed (a lone letter or digit is a token, so "Phase 1" and "Phase 2" differ).

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};
use wg_model::entity::Task;
use wg_model::slice::TaskSlice;
use wg_types::{Id, TaskStatus, WgError};

/// The default of `tasks.duplicate_threshold`.
pub const DEFAULT_THRESHOLD: f64 = 0.6;

/// The most candidates a refusal or warning lists.
const MAX_CANDIDATES: usize = 5;

/// Words that carry no meaning about what a task is.
const STOP_WORDS: [&str; 36] = [
    "a", "an", "of", "to", "in", "on", "is", "it", "or", "be", "as", "at", "by", "the", "and",
    "for", "with", "from", "that", "this", "into", "when", "then", "are", "was", "not", "can",
    "should", "must", "will", "its", "has", "have", "via", "any", "all",
];

/// What the workspace does with a create that matches an open task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateMode {
    /// Refuse the create and list the matches.
    Refuse,
    /// Create the task and return the matches as a warning.
    Warn,
    /// Do not look.
    Off,
}

/// The workspace's duplicate guard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DuplicateGuard {
    pub mode: DuplicateMode,
    pub threshold: f64,
}

/// An open task a new task resembles.
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateCandidate {
    pub key: String,
    pub title: String,
    pub status: TaskStatus,
    /// The overlap, 0 to 1, rounded to two decimals.
    pub score: f64,
}

/// Reads `tasks.duplicate_check` and `tasks.duplicate_threshold`; an absent or unknown value is
/// the default (off, threshold 0.6).
#[must_use]
pub fn duplicate_guard(config: &Map<String, Value>) -> DuplicateGuard {
    let tasks = config.get("tasks");
    let mode = match tasks
        .and_then(|tasks| tasks.get("duplicate_check"))
        .and_then(Value::as_str)
    {
        Some("warn") => DuplicateMode::Warn,
        Some("refuse") => DuplicateMode::Refuse,
        _ => DuplicateMode::Off,
    };
    let threshold = tasks
        .and_then(|tasks| tasks.get("duplicate_threshold"))
        .and_then(Value::as_f64)
        .filter(|threshold| threshold.is_finite())
        .unwrap_or(DEFAULT_THRESHOLD)
        .clamp(0.0, 1.0);
    DuplicateGuard { mode, threshold }
}

/// The normalised tokens of a text.
fn tokens(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty() && !STOP_WORDS.contains(word))
        .map(|word| {
            if word.chars().count() > 3 && word.ends_with('s') && !word.ends_with("ss") {
                word.chars().take(word.chars().count() - 1).collect()
            } else {
                word.to_owned()
            }
        })
        .collect()
}

/// Jaccard overlap of two token sets; two empty sets do not overlap.
fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / union as f64
}

/// The overlap of two tasks' titles and descriptions, 0 to 1.
#[must_use]
pub fn similarity(
    (title_a, description_a): (&str, &str),
    (title_b, description_b): (&str, &str),
) -> f64 {
    let (head_a, head_b) = (tokens(title_a), tokens(title_b));
    let all_a: BTreeSet<String> = head_a.union(&tokens(description_a)).cloned().collect();
    let all_b: BTreeSet<String> = head_b.union(&tokens(description_b)).cloned().collect();
    (jaccard(&head_a, &head_b) + jaccard(&all_a, &all_b)) / 2.0
}

/// Whether `task` counts as an open task a new one can duplicate.
fn is_open(task: &Task) -> bool {
    !matches!(task.status, TaskStatus::Accepted | TaskStatus::Cancelled)
        && task.archived_at.is_none()
}

/// Whether `task` is in the scope of the new task: it shares a repository or a project with it, or,
/// when the new task has neither, has neither itself.
fn in_scope(tasks: &TaskSlice, task: &Task, repositories: &[Id], projects: &[Id]) -> bool {
    let task_repositories: BTreeSet<Id> = tasks
        .repositories_of(task.id)
        .map(|row| row.repository_id)
        .collect();
    let task_projects: BTreeSet<Id> = tasks
        .projects_of(task.id)
        .map(|row| row.project_id)
        .collect();
    if repositories.is_empty() && projects.is_empty() {
        return task_repositories.is_empty() && task_projects.is_empty();
    }
    repositories.iter().any(|id| task_repositories.contains(id))
        || projects.iter().any(|id| task_projects.contains(id))
}

/// The open tasks of `workspace` in the new task's repositories or projects whose overlap with it
/// reaches `threshold`, best first (ties by key), at most five.
#[must_use]
pub fn find_duplicates(
    tasks: &TaskSlice,
    workspace: Id,
    (title, description): (&str, &str),
    (repositories, projects): (&[Id], &[Id]),
    threshold: f64,
) -> Vec<DuplicateCandidate> {
    let mut found: Vec<DuplicateCandidate> = tasks
        .tasks
        .values()
        .filter(|task| task.workspace_id == workspace && is_open(task))
        .filter(|task| in_scope(tasks, task, repositories, projects))
        .map(|task| DuplicateCandidate {
            key: task.key.to_string(),
            title: task.title.clone(),
            status: task.status,
            score: (similarity((title, description), (&task.title, &task.description)) * 100.0)
                .round()
                / 100.0,
        })
        .filter(|candidate| candidate.score >= threshold)
        .collect();
    found.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.key.cmp(&b.key)));
    found.truncate(MAX_CANDIDATES);
    found
}

/// The candidates as a list of `{key, title, status, score}` objects.
fn candidates_json(candidates: &[DuplicateCandidate]) -> Value {
    Value::Array(
        candidates
            .iter()
            .map(|c| {
                json!({
                    "key": c.key,
                    "title": c.title,
                    "status": c.status.as_str(),
                    "score": c.score,
                })
            })
            .collect(),
    )
}

/// One line per candidate: key, title, status, score.
fn candidate_lines(candidates: &[DuplicateCandidate]) -> String {
    candidates
        .iter()
        .map(|c| {
            format!(
                "  {} \"{}\" [{}] similarity {:.2}",
                c.key, c.title, c.status, c.score
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The warning of a create that went ahead beside similar open tasks.
#[must_use]
pub fn duplicates_warning(candidates: &[DuplicateCandidate]) -> String {
    format!(
        "similar open tasks exist; check them before adding more:\n{}",
        candidate_lines(candidates)
    )
}

/// Refuses a create that resembles open tasks: the reply lists them and says what to do. A
/// non-blank `distinct_reason` lets the create through.
pub fn refuse_duplicates(
    candidates: &[DuplicateCandidate],
    distinct_reason: Option<&str>,
) -> Result<(), WgError> {
    if candidates.is_empty() || distinct_reason.is_some_and(|reason| !reason.trim().is_empty()) {
        return Ok(());
    }
    Err(WgError::validation(format!(
        "similar open tasks exist:\n{}\nAdd a comment to the existing task instead, or create this \
         one anyway with a distinct_reason (CLI: --distinct-reason) that says why it is a different \
         task",
        candidate_lines(candidates)
    ))
    .detail("reason", "duplicate_task")
    .detail("candidates", candidates_json(candidates)))
}
