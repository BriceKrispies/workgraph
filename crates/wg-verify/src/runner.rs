//! The pure side of running one check: the environment and log name a check gets, the manifest
//! entry that gives it a working directory, and the result row it leaves (`_execute_check` in
//! `verification/service.py`).
//!
//! The shell resolves the executable, spawns the process, reads the log and passes the facts in;
//! everything it records about the check is decided here.

use wg_model::entity::JsonObject;
use wg_types::{Id, VerificationStatus};

use crate::config::CheckSpec;
use crate::outcome::{ProcessOutcome, check_signatures, check_status};

/// The most characters of a check's output kept in its result row.
const STORED_EXCERPT_CHARS: usize = 8000;

/// The lines of a check's output kept as its excerpt.
const EXCERPT_LINES: usize = 40;

/// The longest message of a check that could not start, in its failure signature.
const START_SIGNATURE_CHARS: usize = 120;

/// The log file name of a check: its name with every character outside `[A-Za-z0-9._-]` replaced
/// by `_`, and `.log` appended.
pub fn log_file_name(check_name: &str) -> String {
    let safe: String = check_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.log")
}

/// The variable that names a repository's checkout: `WORKGRAPH_REPO_<NAME>_PATH`.
fn repository_variable(repository: &str) -> String {
    let key: String = repository
        .to_uppercase()
        .chars()
        .map(|c| {
            if matches!(c, 'A'..='Z' | '0'..='9') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("WORKGRAPH_REPO_{key}_PATH")
}

fn text<'a>(entry: &'a JsonObject, key: &str) -> Option<&'a str> {
    entry.get(key).and_then(serde_json::Value::as_str)
}

/// The manifest entry a check's `repository` reference names, by repository id or name.
pub fn manifest_entry<'a>(entries: &'a [JsonObject], reference: &str) -> Option<&'a JsonObject> {
    entries.iter().find(|entry| {
        text(entry, "repository_id") == Some(reference)
            || text(entry, "repository") == Some(reference)
    })
}

/// The identity of the run a check belongs to.
#[derive(Debug, Clone, Copy)]
pub struct RunIdentity<'a> {
    pub task_id: Id,
    pub task_key: &'a str,
    pub run_id: Id,
    pub manifest_path: &'a str,
}

/// The variables a check adds to the base environment, in the order they apply (a later one
/// replaces an earlier one of the same name): the run's identity, the check's own `env`, then the
/// checkout path of every repository in the manifest.
pub fn check_environment(
    run: &RunIdentity<'_>,
    spec: &CheckSpec,
    entries: &[JsonObject],
) -> Vec<(String, String)> {
    let identity = [
        ("WORKGRAPH_TASK_ID".to_owned(), run.task_id.to_string()),
        ("WORKGRAPH_TASK_KEY".to_owned(), run.task_key.to_owned()),
        (
            "WORKGRAPH_VERIFICATION_RUN_ID".to_owned(),
            run.run_id.to_string(),
        ),
        (
            "WORKGRAPH_MANIFEST".to_owned(),
            run.manifest_path.to_owned(),
        ),
    ];
    let checkouts = entries.iter().filter_map(|entry| {
        let path = text(entry, "checkout_path").filter(|path| !path.is_empty())?;
        let repository = text(entry, "repository")?;
        Some((repository_variable(repository), path.to_owned()))
    });
    identity
        .into_iter()
        .chain(spec.env.iter().cloned())
        .chain(checkouts)
        .collect()
}

/// The last characters of `output`, at most `limit` of them.
fn last_chars(output: &str, limit: usize) -> String {
    let count = output.chars().count();
    output.chars().skip(count.saturating_sub(limit)).collect()
}

/// The result of one check, as the run records it.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutedCheck {
    pub spec: CheckSpec,
    pub status: VerificationStatus,
    pub exit_code: Option<i64>,
    pub duration_ms: i64,
    pub timed_out: bool,
    /// The tail of the output, at most 8000 characters.
    pub excerpt: String,
    pub signatures: Vec<String>,
    /// The check wrote a log file, which the run registers as an artifact.
    pub logged: bool,
    pub repository_id: Option<Id>,
}

impl ExecutedCheck {
    /// A check whose repository is not in the revision manifest, or has no checkout.
    pub fn missing_repository(spec: &CheckSpec) -> Self {
        Self {
            spec: spec.clone(),
            status: VerificationStatus::Error,
            exit_code: None,
            duration_ms: 0,
            timed_out: false,
            excerpt: format!(
                "repository {} is not part of the revision manifest",
                spec.repository.as_deref().unwrap_or_default()
            ),
            signatures: vec!["error:repository missing from manifest".to_owned()],
            logged: false,
            repository_id: None,
        }
    }

    /// A check whose process could not be started.
    pub fn not_started(spec: &CheckSpec, repository_id: Option<Id>, message: &str) -> Self {
        Self {
            spec: spec.clone(),
            status: VerificationStatus::Error,
            exit_code: None,
            duration_ms: 0,
            timed_out: false,
            excerpt: last_chars(message, STORED_EXCERPT_CHARS),
            signatures: vec![format!(
                "error:{}",
                message
                    .chars()
                    .take(START_SIGNATURE_CHARS)
                    .collect::<String>()
            )],
            logged: false,
            repository_id,
        }
    }

    /// A check whose process ran to an end. `tail` is the last lines the process wrote and
    /// `output` the lines the signatures are read from: the whole log when it could be read,
    /// else the tail.
    pub fn finished(
        spec: &CheckSpec,
        repository_id: Option<Id>,
        process: &ProcessOutcome,
        duration_ms: i64,
        tail: &[String],
        output: &[String],
        logged: bool,
    ) -> Self {
        let kept = tail.len().saturating_sub(EXCERPT_LINES);
        Self {
            spec: spec.clone(),
            status: check_status(spec, process),
            exit_code: process.exit_code,
            duration_ms,
            timed_out: process.timed_out,
            excerpt: last_chars(&tail[kept..].join("\n"), STORED_EXCERPT_CHARS),
            signatures: check_signatures(process.ok, process.timed_out, output, spec.parser),
            logged,
            repository_id,
        }
    }
}
