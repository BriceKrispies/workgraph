//! Correlation and binding (the decisions of `_resolve_binding` and the task matching in `ingest.py`).
//!
//! A Claude Code session is never authoritative for Workgraph: it is bound to an attempt or a task only
//! through Workgraph's own launch context (the environment), an explicit operator binding, or a `cwd`
//! prefix an operator registered. The shell does the lookups and passes the matching rows in
//! [`BindingFacts`]; the precedence and the path rules are decided here.

use crate::hook::task_keys_in;

/// Where a session's binding came from. The text is the value the store keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingSource {
    /// The environment named an attempt Workgraph launched.
    WorkgraphLaunched,
    /// An operator binding matched by session id or by `cwd` prefix.
    ExplicitBinding,
    /// The environment named only a task.
    Environment,
    /// Nothing matched.
    Unbound,
}

impl BindingSource {
    /// The stored text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkgraphLaunched => "workgraph_launched",
            Self::ExplicitBinding => "explicit_binding",
            Self::Environment => "environment",
            Self::Unbound => "unbound",
        }
    }
}

/// An attempt the environment names, with the workspace and task it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptRow {
    pub workspace_id: String,
    pub attempt_id: String,
    pub task_id: String,
}

/// A workspace and task an operator binding or the environment points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetRow {
    pub workspace_id: String,
    pub task_id: String,
}

/// An active binding with a `cwd` prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CwdBinding {
    pub target: TargetRow,
    /// The registered prefix; `None` never matches.
    pub prefix: Option<String>,
}

/// The rows the shell looked up for one hook payload.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BindingFacts {
    /// The attempt named by `WORKGRAPH_ATTEMPT_ID`, when it exists.
    pub env_attempt: Option<AttemptRow>,
    /// The active binding for the payload's session id, when there is one.
    pub session_binding: Option<TargetRow>,
    /// Active `cwd` bindings, in the order the store returns them.
    pub cwd_bindings: Vec<CwdBinding>,
    /// The task named by `WORKGRAPH_TASK_ID`, when it exists.
    pub env_task: Option<TargetRow>,
}

/// The outcome of [`resolve_binding`]. Each field is `None` when the binding does not set it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub workspace_id: Option<String>,
    pub attempt_id: Option<String>,
    pub task_id: Option<String>,
    pub source: BindingSource,
}

impl Resolved {
    fn unbound() -> Self {
        Self {
            workspace_id: None,
            attempt_id: None,
            task_id: None,
            source: BindingSource::Unbound,
        }
    }

    fn to_target(target: &TargetRow, source: BindingSource) -> Self {
        Self {
            workspace_id: Some(target.workspace_id.clone()),
            attempt_id: None,
            task_id: Some(target.task_id.clone()),
            source,
        }
    }
}

/// Resolves the binding of a session. Precedence: the launched attempt, then the operator binding for the
/// session id, then a `cwd` binding, then the environment's task, else unbound. `session_id` and `cwd` are
/// the payload's values, `None` when absent or empty.
pub fn resolve_binding(
    facts: &BindingFacts,
    session_id: Option<&str>,
    cwd: Option<&str>,
) -> Resolved {
    let launched = facts.env_attempt.as_ref().map(|attempt| Resolved {
        workspace_id: Some(attempt.workspace_id.clone()),
        attempt_id: Some(attempt.attempt_id.clone()),
        task_id: Some(attempt.task_id.clone()),
        source: BindingSource::WorkgraphLaunched,
    });
    let by_session = || {
        session_id
            .filter(|id| !id.is_empty())
            .and(facts.session_binding.as_ref())
            .map(|target| Resolved::to_target(target, BindingSource::ExplicitBinding))
    };
    let by_cwd = || {
        cwd.filter(|path| !path.is_empty()).and_then(|path| {
            let cwd_text = windows_path(path).to_lowercase();
            facts
                .cwd_bindings
                .iter()
                .find(|binding| {
                    binding
                        .prefix
                        .as_deref()
                        .is_some_and(|prefix| cwd_under(&cwd_text, prefix))
                })
                .map(|binding| Resolved::to_target(&binding.target, BindingSource::ExplicitBinding))
        })
    };
    let by_environment = || {
        facts
            .env_task
            .as_ref()
            .map(|target| Resolved::to_target(target, BindingSource::Environment))
    };
    launched
        .or_else(by_session)
        .or_else(by_cwd)
        .or_else(by_environment)
        .unwrap_or_else(Resolved::unbound)
}

/// Whether the normalised `cwd` is the registered prefix or lies under it. Both are lowercased Windows paths.
fn cwd_under(cwd: &str, prefix: &str) -> bool {
    let prefix = windows_path(prefix).to_lowercase();
    if prefix.is_empty() {
        return false;
    }
    let separator = if prefix.contains('\\') { '\\' } else { '/' };
    let trimmed = prefix.trim_end_matches(['\\', '/']).to_owned();
    cwd == prefix || cwd.starts_with(&format!("{trimmed}{separator}"))
}

/// `str(pathlib.Path(text))` on Windows: forward slashes become backslashes, repeated and trailing separators
/// go, a drive root keeps its separator, and an empty path is `.`.
pub fn windows_path(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    let rooted = replaced.starts_with('\\');
    let body = replaced
        .split('\\')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\\");
    let drive_root = body.len() == 2 && body.ends_with(':') && replaced.ends_with('\\');
    match (rooted, body.is_empty()) {
        (true, true) => "\\".to_owned(),
        (true, false) => format!("\\{body}"),
        (false, true) => ".".to_owned(),
        (false, false) if drive_root => format!("{body}\\"),
        (false, false) => body,
    }
}

/// The `Task-ID` key a transcript's prompt names: the first marker only, since a brief leads with its own.
pub fn transcript_task_key(keys: &[String]) -> Option<&str> {
    keys.first().map(String::as_str)
}

/// The task a transcript binds to: the one match of its first key, when exactly one task in scope has it.
pub fn transcript_task<T>(keys: &[String], matches: Vec<T>) -> Option<T> {
    transcript_task_key(keys).and_then(|_| single(matches))
}

/// The one element of a list of exactly one element.
pub fn single<T>(items: Vec<T>) -> Option<T> {
    if items.len() == 1 {
        items.into_iter().next()
    } else {
        None
    }
}

/// The first `Task-ID` key in an internal task's subject or description that `is_known` accepts, which the
/// shell answers from the workspace's task keys.
pub fn mapped_task_key(
    subject: Option<&str>,
    description: Option<&str>,
    is_known: impl Fn(&str) -> bool,
) -> Option<String> {
    let text = format!("{} {}", subject.unwrap_or(""), description.unwrap_or(""));
    task_keys_in(&text).into_iter().find(|key| is_known(key))
}
