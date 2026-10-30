//! wg-store: rusqlite connection setup, migrations, row codecs, SQL.
//!
//! This is the base of the store (SH-01). The pure half lives in `wg-rows` (row and event
//! codecs, SQL text and parameters as values); this crate is the glue to SQLite:
//!
//! * [`Store`]: a connection configured as `persistence/db.py` configures one (WAL, a 15 second
//!   busy timeout, foreign keys on, `synchronous=NORMAL`), with read transactions and
//!   `BEGIN IMMEDIATE` write transactions;
//! * [`Session`]: the open transaction a body works in. It binds and runs statements, applies a
//!   decision's writes and events ([`Session::apply_decision`], run as one transaction by
//!   [`Store::commit_decision`]), and reads the append-only `events` log.
//!
//! The migration runner is [`Store::upgrade`]; later tasks add the per-entity queries on top of this base.

mod acceptance;
mod jobs;
mod migrate;
pub mod query;
mod repos;
mod run;
mod session;
mod stacks;
mod task_world;
mod tasks;

use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior};
use wg_model::Decision;
use wg_types::WgError;

use session::db;

pub use query::{many, one};
pub use run::AttemptPage;
pub use session::{Applied, Session};
pub use wg_rows::migrations::head_revision;
pub use wg_rows::run_sql::{RecordedHead, WorktreeWithRepository};
pub use wg_rows::task_world_sql::TaskWorld;
pub use wg_rows::{EventQuery, EventRecord, Raw, Statement, TableRow};

/// Python's `BUSY_TIMEOUT_MS`.
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(15_000);

/// Where a database lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// A database file, created with its parent directories.
    File(PathBuf),
    /// A private in-memory database (tests, scratch work).
    Memory,
}

/// An open database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens the database and applies `_configure_connection`'s pragmas.
    pub fn open(location: &Location) -> Result<Store, WgError> {
        let conn = match location {
            Location::File(path) => {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent).map_err(|err| {
                        WgError::workgraph(format!("cannot create {}: {err}", parent.display()))
                    })?;
                }
                db!(Connection::open(path))?
            }
            Location::Memory => db!(Connection::open_in_memory())?,
        };
        db!(conn.busy_timeout(BUSY_TIMEOUT))?;
        // `journal_mode` answers with the mode now in force, so it is a query.
        db!(conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0)))?;
        db!(conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;"))?;
        Ok(Store { conn })
    }

    /// Runs `body` in a read transaction (a consistent snapshot); nothing is committed.
    pub fn read<T>(
        &mut self,
        body: impl FnOnce(&mut Session<'_>) -> Result<T, WgError>,
    ) -> Result<T, WgError> {
        let tx = db!(self
            .conn
            .transaction_with_behavior(TransactionBehavior::Deferred))?;
        body(&mut Session { conn: &tx })
    }

    /// Runs `body` in a `BEGIN IMMEDIATE` transaction, so the write lock is taken up front. Commits
    /// when `body` returns `Ok`; any error rolls back every change `body` made.
    pub fn write<T>(
        &mut self,
        body: impl FnOnce(&mut Session<'_>) -> Result<T, WgError>,
    ) -> Result<T, WgError> {
        let tx = db!(self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate))?;
        let value = body(&mut Session { conn: &tx })?;
        db!(tx.commit())?;
        Ok(value)
    }

    /// Copies the whole database into a new file at `target` with SQLite's online backup API, so
    /// the copy is consistent while other connections write (`_sqlite_backup`).
    pub fn backup_to(&mut self, target: &std::path::Path) -> Result<(), WgError> {
        let mut dst = db!(Connection::open(target))?;
        let backup = db!(rusqlite::backup::Backup::new(&self.conn, &mut dst))?;
        db!(backup.step(-1))?;
        Ok(())
    }

    /// Applies a decision's writes and events in one write transaction. The caller performs the
    /// decision's effects afterwards.
    pub fn commit_decision<R>(&mut self, decision: &Decision<R>) -> Result<Applied, WgError> {
        self.write(|session| session.apply_decision(decision))
    }
}
