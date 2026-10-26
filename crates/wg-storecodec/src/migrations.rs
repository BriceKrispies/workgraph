//! The migration chain as values: the revision list, each revision's DDL, the `alembic_version`
//! statements, the data steps that move rows, and the ordering of a run. `wg-store` executes the
//! statements and reads and writes rows; nothing here touches a database.
//!
//! The runner reads and writes Alembic's `alembic_version` table with Alembic's revision ids, so
//! either implementation can open a database the other migrated.

use std::collections::HashSet;

use serde_json::Value;
use wg_types::{ErrorCode, Id, Timestamp, WgError};

use crate::codec::Raw;
use crate::sql::Statement;

/// One revision of the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision {
    /// The Alembic revision id (`"0001"`).
    pub id: &'static str,
    /// The revision this one upgrades from; `None` for the first.
    pub down: Option<&'static str>,
    /// The DDL file: statements separated by `----` lines (see [`Revision::statements`]).
    pub ddl: &'static str,
    /// The row-moving step that runs around the DDL, if the revision has one.
    pub data: Option<DataStep>,
}

impl Revision {
    /// The DDL statements in the order Alembic's upgrade runs them.
    pub fn statements(&self) -> Vec<&'static str> {
        split_statements(self.ddl)
    }
}

/// A revision's row-moving step. Each one is the part of an Alembic `upgrade()` that reads or
/// writes rows rather than changing the schema; the runner performs its database access and calls
/// the pure function named here for the rows themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataStep {
    /// Revision `0009`: each task's legacy `external_refs` JSON becomes `task_links` rows.
    ExternalRefsToTaskLinks,
}

/// Reads the rows [`external_ref_links`] needs: `(task id, external_refs JSON text)`. Runs before
/// the `0009` DDL drops the column.
pub const SELECT_EXTERNAL_REFS: &str = "SELECT id, external_refs FROM tasks";

/// Alembic's bookkeeping table, created before the first revision runs.
pub const CREATE_VERSION_TABLE: &str = "CREATE TABLE IF NOT EXISTS alembic_version (\n\tversion_num VARCHAR(32) NOT NULL, \n\tCONSTRAINT alembic_version_pkc PRIMARY KEY (version_num)\n)";

/// Reads the recorded revisions (no row when the database is unmigrated).
pub const SELECT_VERSION: &str = "SELECT version_num FROM alembic_version";

/// Records the first revision.
pub const INSERT_VERSION: &str = "INSERT INTO alembic_version (version_num) VALUES (?1)";

/// Moves the recorded revision from `?2` to `?1`.
pub const UPDATE_VERSION: &str =
    "UPDATE alembic_version SET version_num = ?1 WHERE version_num = ?2";

/// The revision chain, oldest first. Each DDL file holds the statements Alembic's upgrade ran
/// (captured from the Python revisions and checked against `golden/db/rev-NNNN.db`).
pub const REVISIONS: &[Revision] = &[
    Revision {
        id: "0001",
        down: None,
        ddl: include_str!("../ddl/0001.sql"),
        data: None,
    },
    Revision {
        id: "0002",
        down: Some("0001"),
        ddl: include_str!("../ddl/0002.sql"),
        data: None,
    },
    Revision {
        id: "0003",
        down: Some("0002"),
        ddl: include_str!("../ddl/0003.sql"),
        data: None,
    },
    Revision {
        id: "0004",
        down: Some("0003"),
        ddl: include_str!("../ddl/0004.sql"),
        data: None,
    },
    Revision {
        id: "0005",
        down: Some("0004"),
        ddl: include_str!("../ddl/0005.sql"),
        data: None,
    },
    Revision {
        id: "0006",
        down: Some("0005"),
        ddl: include_str!("../ddl/0006.sql"),
        data: None,
    },
    Revision {
        id: "0007",
        down: Some("0006"),
        ddl: include_str!("../ddl/0007.sql"),
        data: None,
    },
    Revision {
        id: "0008",
        down: Some("0007"),
        ddl: include_str!("../ddl/0008.sql"),
        data: None,
    },
    Revision {
        id: "0009",
        down: Some("0008"),
        ddl: include_str!("../ddl/0009.sql"),
        data: Some(DataStep::ExternalRefsToTaskLinks),
    },
    Revision {
        id: "0010",
        down: Some("0009"),
        ddl: include_str!("../ddl/0010.sql"),
        data: None,
    },
    Revision {
        id: "0011",
        down: Some("0010"),
        ddl: include_str!("../ddl/0011.sql"),
        data: None,
    },
];

/// Splits a DDL file into statements: blocks between `----` lines, comment-only blocks dropped.
pub fn split_statements(text: &'static str) -> Vec<&'static str> {
    text.split("\n----\n")
        .map(str::trim)
        .filter(|block| !block.is_empty() && !block.lines().all(|line| line.starts_with("--")))
        .collect()
}

/// The newest revision id.
pub fn head_revision() -> Option<&'static str> {
    REVISIONS.last().map(|revision| revision.id)
}

/// The revisions to run to take a database at `current` (`None`: unmigrated) to `target` (a
/// revision id or `"head"`), oldest first. Already at the target is an empty list; a target older
/// than `current` is an error (downgrades are not ported).
pub fn upgrade_path(
    current: Option<&str>,
    target: &str,
) -> Result<Vec<&'static Revision>, WgError> {
    let fail = |message: String| WgError::new(ErrorCode::WorkgraphError, message);
    let position = |id: &str| REVISIONS.iter().position(|revision| revision.id == id);
    let end = if target == "head" {
        REVISIONS.len()
    } else {
        position(target)
            .map(|index| index + 1)
            .ok_or_else(|| fail(format!("can't locate revision identified by '{target}'")))?
    };
    let start = match current {
        None => 0,
        Some(id) => position(id)
            .map(|index| index + 1)
            .ok_or_else(|| fail(format!("can't locate revision identified by '{id}'")))?,
    };
    if start > end {
        return Err(fail(format!(
            "cannot downgrade from revision '{}' to '{target}'",
            current.unwrap_or("")
        )));
    }
    Ok(REVISIONS[start..end].iter().collect())
}

/// The insert `0009`'s data step runs for each new link row: the statement Python's `bulk_insert`
/// issues, with the column order of `contracts/schema-0011.sql`.
pub const INSERT_TASK_LINK: &str = "INSERT INTO task_links (id, task_id, kind, provider, url, normalized_url, repo_full_name, number, repository_id, sources, title, created_by, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)";

/// Migration `0009`'s rows (Python's `upgrade()` of that revision): for each task, one
/// `external_ref` link per distinct `system:id` in its legacy `external_refs` JSON array, as an
/// [`INSERT_TASK_LINK`] statement. Entries that are not objects, or lack a system or an id, are
/// skipped; so is a repeated `system:id`.
///
/// `tasks` is `(task id, external_refs text)` in table order. The task id is copied as text, as
/// Python copies it (fixture databases hold ids that are not UUIDs). `new_id` supplies the link
/// ids (the shell generates them, as everywhere else) and `now` stamps both timestamps.
pub fn external_ref_links(
    tasks: &[(String, Option<String>)],
    now: Timestamp,
    new_id: &mut dyn FnMut() -> Id,
) -> Result<Vec<Statement>, WgError> {
    let stamp = now.format_db();
    let mut statements = Vec::new();
    for (task_id, raw) in tasks {
        let refs = match raw.as_deref() {
            None | Some("") => Vec::new(),
            Some(text) => match serde_json::from_str::<Value>(text).map_err(|err| {
                WgError::new(
                    ErrorCode::WorkgraphError,
                    format!("task {task_id}: external_refs is not JSON: {err}"),
                )
            })? {
                Value::Array(items) => items,
                // Python iterates a string or an object without error, yielding no dict entries.
                Value::Object(_) | Value::String(_) => Vec::new(),
                other => {
                    return Err(WgError::new(
                        ErrorCode::WorkgraphError,
                        format!("task {task_id}: external_refs is not a list: {other}"),
                    ));
                }
            },
        };
        let mut seen = HashSet::new();
        for item in refs {
            let Value::Object(entry) = item else {
                continue;
            };
            let system = legacy_text(entry.get("system"));
            let ref_id = legacy_text(entry.get("id"));
            let normalized = format!("{system}:{ref_id}");
            if system.is_empty() || ref_id.is_empty() || !seen.insert(normalized.clone()) {
                continue;
            }
            statements.push(Statement {
                sql: INSERT_TASK_LINK.to_owned(),
                params: vec![
                    Raw::Text(new_id().to_string()),
                    Raw::Text(task_id.clone()),
                    Raw::Text("external_ref".to_owned()),
                    Raw::Text(system),
                    Raw::Text(legacy_text(entry.get("url"))),
                    Raw::Text(normalized),
                    Raw::Text(String::new()),
                    Raw::Null,
                    Raw::Null,
                    Raw::Text(r#"["migrated"]"#.to_owned()),
                    Raw::Text(ref_id),
                    Raw::Text("system:migration".to_owned()),
                    Raw::Text(stamp.clone()),
                    Raw::Text(stamp.clone()),
                ],
            });
        }
    }
    Ok(statements)
}

/// Python's `str(value or "")` for a JSON value read from a legacy `external_refs` entry: falsy
/// values (null, false, zero, empty text or list) become the empty string.
fn legacy_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
        Some(Value::Bool(true)) => "True".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) if number.as_f64() == Some(0.0) => String::new(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Array(items)) if items.is_empty() => String::new(),
        Some(Value::Object(entries)) if entries.is_empty() => String::new(),
        Some(other) => other.to_string(),
    }
}
