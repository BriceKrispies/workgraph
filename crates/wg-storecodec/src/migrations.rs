//! The migration chain as values: the revision list, each revision's DDL, the `alembic_version`
//! statements and the ordering of a run. `wg-store` executes the statements; nothing here touches a
//! database.
//!
//! The runner reads and writes Alembic's `alembic_version` table with Alembic's revision ids, so
//! either implementation can open a database the other migrated.

use wg_types::{ErrorCode, WgError};

/// One revision of the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Revision {
    /// The Alembic revision id (`"0001"`).
    pub id: &'static str,
    /// The revision this one upgrades from; `None` for the first.
    pub down: Option<&'static str>,
    /// The DDL file: statements separated by `----` lines (see [`Revision::statements`]).
    pub ddl: &'static str,
}

impl Revision {
    /// The DDL statements in the order Alembic's upgrade runs them.
    pub fn statements(&self) -> Vec<&'static str> {
        split_statements(self.ddl)
    }
}

/// Alembic's bookkeeping table, created before the first revision runs.
pub const CREATE_VERSION_TABLE: &str = "CREATE TABLE IF NOT EXISTS alembic_version (\n\tversion_num VARCHAR(32) NOT NULL, \n\tCONSTRAINT alembic_version_pkc PRIMARY KEY (version_num)\n)";

/// Reads the recorded revisions (no row when the database is unmigrated).
pub const SELECT_VERSION: &str = "SELECT version_num FROM alembic_version";

/// Records the first revision.
pub const INSERT_VERSION: &str = "INSERT INTO alembic_version (version_num) VALUES (?1)";

/// Moves the recorded revision from `?2` to `?1`.
pub const UPDATE_VERSION: &str =
    "UPDATE alembic_version SET version_num = ?1 WHERE version_num = ?2";

/// The revision chain, oldest first.
pub const REVISIONS: &[Revision] = &[Revision {
    id: "0001",
    down: None,
    ddl: include_str!("../ddl/0001.sql"),
}];

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
