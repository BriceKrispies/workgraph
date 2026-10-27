//! One check of a verification run (`VerificationService._execute_check`): where it runs, the
//! environment it gets, and how its process's outcome becomes a result. Starting the process and
//! reading its log are the shell's.

use serde_json::Value;
use wg_model::entity::JsonObject;
use wg_types::{Id, VerificationStatus};
use wg_verify::{CheckSpec, ProcessOutcome, check_signatures, check_status};

use super::run::{Executed, LogFile, Prepared};

/// Where a check runs.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckPlace {
    /// The check names a repository the manifest does not hold: it errors without running.
    Missing(Box<Executed>),
    /// The directory to run in (`None`: the run's own directory) and the repository it belongs to.
    Run {
        cwd: Option<String>,
        repository_id: Option<Id>,
    },
}

/// The manifest entry a check's repository names (by id or by name).
fn entry_for<'a>(entries: &'a [JsonObject], repository: &str) -> Option<&'a JsonObject> {
    entries.iter().find(|entry| {
        ["repository_id", "repository"]
            .iter()
            .any(|key| entry.get(*key).and_then(Value::as_str) == Some(repository))
    })
}

/// An error result: the check could not be started or judged.
#[must_use]
pub fn errored(spec: &CheckSpec, repository_id: Option<Id>, message: &str) -> Executed {
    Executed {
        spec: spec.clone(),
        status: VerificationStatus::Error,
        exit_code: None,
        duration_ms: 0,
        timed_out: false,
        excerpt: message.to_owned(),
        signatures: vec![format!(
            "error:{}",
            message.chars().take(120).collect::<String>()
        )],
        log: None,
        repository_id,
    }
}

/// Where a check runs, from the manifest entries.
#[must_use]
pub fn check_place(entries: &[JsonObject], spec: &CheckSpec) -> CheckPlace {
    let Some(repository) = spec.repository.as_deref() else {
        return CheckPlace::Run {
            cwd: None,
            repository_id: None,
        };
    };
    let found = entry_for(entries, repository).and_then(|entry| {
        let path = entry.get("checkout_path").and_then(Value::as_str)?;
        let id = entry
            .get("repository_id")
            .and_then(Value::as_str)
            .and_then(|text| wg_types::parse_id(text).ok());
        (!path.is_empty()).then(|| (path.to_owned(), id))
    });
    match found {
        Some((path, id)) => CheckPlace::Run {
            cwd: Some(path),
            repository_id: id,
        },
        None => CheckPlace::Missing(Box::new(Executed {
            signatures: vec!["error:repository missing from manifest".to_owned()],
            ..errored(
                spec,
                None,
                &format!("repository {repository} is not part of the revision manifest"),
            )
        })),
    }
}

/// The environment a check runs with, on top of the minimal one: the task and run, the manifest
/// file, the check's own variables, then each checked-out repository's path.
#[must_use]
pub fn check_env(
    prepared: &Prepared,
    spec: &CheckSpec,
    manifest_path: &str,
) -> Vec<(String, String)> {
    let base = [
        ("WORKGRAPH_TASK_ID".to_owned(), prepared.task_id.to_string()),
        ("WORKGRAPH_TASK_KEY".to_owned(), prepared.task_key.clone()),
        (
            "WORKGRAPH_VERIFICATION_RUN_ID".to_owned(),
            prepared.run_id.to_string(),
        ),
        ("WORKGRAPH_MANIFEST".to_owned(), manifest_path.to_owned()),
    ];
    let repositories = prepared.entries.iter().filter_map(|entry| {
        let path = entry.get("checkout_path").and_then(Value::as_str)?;
        let name = entry.get("repository").and_then(Value::as_str)?;
        (!path.is_empty()).then(|| {
            let key: String = name
                .to_uppercase()
                .chars()
                .map(|c| {
                    if c.is_ascii_uppercase() || c.is_ascii_digit() {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            (format!("WORKGRAPH_REPO_{key}_PATH"), path.to_owned())
        })
    });
    base.into_iter()
        .chain(spec.env.iter().cloned())
        .chain(repositories)
        .collect()
}

/// The log of a check, relative to the artifacts directory, with `/` separators.
#[must_use]
pub fn log_relative(workspace_id: Id, run_id: Id, check: &str) -> String {
    let name: String = check
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{workspace_id}/verification/{run_id}/{name}.log")
}

/// A timeout in seconds as milliseconds (the configuration bounds it to one day).
#[must_use]
pub fn timeout_millis(seconds: f64) -> u64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let millis = (seconds * 1000.0).max(0.0) as u64;
    millis
}

/// The last 40 lines of a check's output.
#[must_use]
pub fn excerpt_of(tail: &[String]) -> String {
    tail[tail.len().saturating_sub(40)..].join("\n")
}

/// What a check process did, as the shell saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessFacts {
    pub ok: bool,
    pub cancelled: bool,
    pub timed_out: bool,
    pub exit_code: Option<i64>,
    pub duration_ms: i64,
    /// The last lines of output.
    pub tail: Vec<String>,
    /// The whole log when it could be read, else the tail.
    pub failure_lines: Vec<String>,
}

/// A check's result from its process's outcome.
#[must_use]
pub fn concluded(
    spec: &CheckSpec,
    repository_id: Option<Id>,
    facts: &ProcessFacts,
    log: Option<LogFile>,
) -> Executed {
    let process = ProcessOutcome {
        ok: facts.ok,
        cancelled: facts.cancelled,
        timed_out: facts.timed_out,
        exit_code: facts.exit_code,
    };
    Executed {
        spec: spec.clone(),
        status: check_status(spec, &process),
        exit_code: facts.exit_code,
        duration_ms: facts.duration_ms,
        timed_out: facts.timed_out,
        excerpt: excerpt_of(&facts.tail),
        signatures: check_signatures(facts.ok, facts.timed_out, &facts.failure_lines, spec.parser),
        log,
        repository_id,
    }
}
