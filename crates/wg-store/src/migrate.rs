//! The migration runner (`persistence/migrate.py`): Alembic's `alembic_version` table, revision
//! ids and upgrade semantics. The chain and its DDL are values in `wg_storecodec::migrations`; this
//! module runs them in one write transaction (SQLite DDL is transactional, so a failed revision
//! leaves the database as it was). A revision's data step reads and writes rows around its DDL.

use rusqlite::params;
use time::OffsetDateTime;
use uuid::Uuid;
use wg_storecodec::Statement;
use wg_storecodec::migrations::{
    CREATE_VERSION_TABLE, DataStep, INSERT_VERSION, SELECT_EXTERNAL_REFS, SELECT_VERSION,
    UPDATE_VERSION, external_ref_links, head_revision, upgrade_path,
};
use wg_types::{Timestamp, WgError};

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
        _ => Err(WgError::workgraph(
            "alembic_version records more than one revision",
        )),
    }
}

/// Migration `0009`'s rows, read before its DDL drops `tasks.external_refs`. Ids are UUIDv4 and
/// the timestamps are the time of the run, as Python's `uuid4()` and `datetime.now(UTC)` are.
fn external_ref_links_of(session: &mut Session<'_>) -> Result<Vec<Statement>, WgError> {
    let conn = session.conn;
    let mut statement = db!(conn.prepare(SELECT_EXTERNAL_REFS))?;
    let tasks = db!(statement
        .query_map([], |row| Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
        )))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>()))?;
    let now = Timestamp::from_offset_datetime(OffsetDateTime::now_utc())?;
    external_ref_links(&tasks, now, std::iter::repeat_with(Uuid::new_v4))
}

/// Runs a revision's data step, returning the statements that write its rows. They run after the
/// revision's DDL.
fn data_statements(
    session: &mut Session<'_>,
    step: Option<DataStep>,
) -> Result<Vec<Statement>, WgError> {
    match step {
        None => Ok(Vec::new()),
        Some(DataStep::ExternalRefsToTaskLinks) => external_ref_links_of(session),
    }
}

impl Store {
    /// `PRAGMA integrity_check`: `"ok"`, or the first problem SQLite reports (`workgraph doctor`).
    pub fn integrity_check(&mut self) -> Result<String, WgError> {
        self.read(|session| {
            db!(session
                .conn
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0)))
        })
    }

    /// `PRAGMA journal_mode`: the mode now in force, lower case (`workgraph doctor`).
    pub fn journal_mode(&mut self) -> Result<String, WgError> {
        self.read(|session| {
            db!(session
                .conn
                .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0)))
        })
    }

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
                let writes = data_statements(session, revision.data)?;
                for statement in revision.statements() {
                    db!(session.conn.execute_batch(statement))?;
                }
                for write in &writes {
                    session.run(write)?;
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
