//! The migration runner (`persistence/migrate.py`): Alembic's `alembic_version` table, revision
//! ids and upgrade semantics. The chain and its DDL are values in `wg_storecodec::migrations`; this
//! module runs them in one write transaction (SQLite DDL is transactional, so a failed revision
//! leaves the database as it was).

use rusqlite::params;
use wg_storecodec::migrations::{
    CREATE_VERSION_TABLE, INSERT_VERSION, SELECT_VERSION, UPDATE_VERSION, head_revision,
    upgrade_path,
};
use wg_types::{ErrorCode, WgError};

use crate::session::db;
use crate::{Session, Store};

/// The recorded revision, `None` for an unmigrated database (no table, or no row).
fn recorded(session: &mut Session<'_>) -> Result<Option<String>, WgError> {
    let conn = session.conn;
    let has_table: bool = db!(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'alembic_version')",
        [],
        |row| row.get(0),
    ))?;
    if !has_table {
        return Ok(None);
    }
    let mut statement = db!(conn.prepare(SELECT_VERSION))?;
    let versions = db!(statement
        .query_map([], |row| row.get::<_, String>(0))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>()))?;
    match versions.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        _ => Err(WgError::new(
            ErrorCode::WorkgraphError,
            "alembic_version records more than one revision",
        )),
    }
}

impl Store {
    /// Python's `current_revision`: the revision in `alembic_version`, `None` when unmigrated.
    pub fn current_revision(&mut self) -> Result<Option<String>, WgError> {
        self.read(recorded)
    }

    /// Python's `is_up_to_date`.
    pub fn is_up_to_date(&mut self) -> Result<bool, WgError> {
        Ok(self.current_revision()?.as_deref() == head_revision())
    }

    /// Python's `upgrade`: runs every revision between the recorded one and `target` (a revision
    /// id or `"head"`) in one transaction, recording each in `alembic_version`. Returns the ids
    /// that ran, oldest first. Like Alembic, it creates `alembic_version` even when there is
    /// nothing to run.
    pub fn upgrade(&mut self, target: &str) -> Result<Vec<&'static str>, WgError> {
        self.write(|session| {
            db!(session.conn.execute_batch(CREATE_VERSION_TABLE))?;
            let mut current = recorded(session)?;
            let mut ran = Vec::new();
            for revision in upgrade_path(current.as_deref(), target)? {
                for statement in revision.statements() {
                    db!(session.conn.execute_batch(statement))?;
                }
                match &current {
                    None => db!(session.conn.execute(INSERT_VERSION, params![revision.id]))?,
                    Some(previous) => db!(session
                        .conn
                        .execute(UPDATE_VERSION, params![revision.id, previous]))?,
                };
                current = Some(revision.id.to_owned());
                ran.push(revision.id);
            }
            Ok(ran)
        })
    }
}
