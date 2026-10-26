//! `RepositoryStatus`.

use wg_json::Value;
use wg_types::Id;

use crate::convention::{id, int, model, text};

/// The OpenAPI component schema this DTO mirrors.
pub const NAME: &str = "RepositoryStatus";

/// The fields in Python declaration order.
pub const FIELDS: [&str; 14] = [
    "repository_id",
    "name",
    "checkout_path",
    "available",
    "branch",
    "head",
    "detached",
    "staged",
    "unstaged",
    "untracked",
    "ahead",
    "behind",
    "is_shallow",
    "error",
];

/// The working-tree status of a repository's primary checkout, as the status service reports it.
///
/// No Rust type for `git status --porcelain` exists yet, so the status code fills this struct in.
/// A repository with no available checkout has `available: false` and an `error`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub checkout_path: Option<String>,
    pub available: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub detached: bool,
    pub staged: Vec<String>,
    pub unstaged: Vec<String>,
    pub untracked: Vec<String>,
    pub ahead: Option<i64>,
    pub behind: Option<i64>,
    pub is_shallow: bool,
    pub error: Option<String>,
}

fn optional_text(value: &Option<String>) -> Value {
    value.as_deref().map_or(Value::Null, text)
}

fn texts(items: &[String]) -> Value {
    Value::List(items.iter().map(|s| text(s)).collect())
}

/// The status of one repository.
pub fn from_model(repository_id: Id, name: &str, status: &Status) -> Value {
    model(
        NAME,
        vec![
            ("repository_id", id(repository_id)),
            ("name", text(name)),
            ("checkout_path", optional_text(&status.checkout_path)),
            ("available", Value::Bool(status.available)),
            ("branch", optional_text(&status.branch)),
            ("head", optional_text(&status.head)),
            ("detached", Value::Bool(status.detached)),
            ("staged", texts(&status.staged)),
            ("unstaged", texts(&status.unstaged)),
            ("untracked", texts(&status.untracked)),
            ("ahead", status.ahead.map_or(Value::Null, int)),
            ("behind", status.behind.map_or(Value::Null, int)),
            ("is_shallow", Value::Bool(status.is_shallow)),
            ("error", optional_text(&status.error)),
        ],
    )
}
