//! wg-store: rusqlite connection setup, migrations, row codecs, SQL.
//!
//! This is the base of the store (SH-01):
//!
//! * [`Store`]: a connection configured as `persistence/db.py` configures one (WAL, a 15 second
//!   busy timeout, foreign keys on, `synchronous=NORMAL`), with read transactions and
//!   `BEGIN IMMEDIATE` write transactions;
//! * [`codec`] and [`rows`]: the column codecs and one row codec per model table;
//! * [`apply`]: [`apply_decision`] applies a decision's writes and events inside one transaction,
//!   and [`Store::commit_decision`] runs it as such;
//! * [`events`]: the append-only `events` log, appended and read back like Python's
//!   `core/events/log.py`.
//!
//! Later tasks add the migration runner and the per-entity queries on top of this base.

pub mod apply;
pub mod codec;
pub mod events;
pub mod rows;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior};
use wg_model::Decision;
use wg_types::{ErrorCode, WgError};

pub use apply::{Applied, apply_decision, apply_write, fetch};
pub use codec::db_error;
pub use events::{EventQuery, EventRecord, append_event, latest_seq, query_events};
pub use rows::TableRow;

/// Python's `BUSY_TIMEOUT_MS`.
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(15_000);

/// An open database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating, with its parent directories) the database file at `path`.
    pub fn open(path: &Path) -> Result<Store, WgError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|err| {
                WgError::new(
                    ErrorCode::WorkgraphError,
                    format!("cannot create {}: {err}", parent.display()),
                )
            })?;
        }
        let conn = Connection::open(path).map_err(|err| db_error(&err))?;
        Store::configure(conn)
    }

    /// An in-memory database (tests, scratch work).
    pub fn open_in_memory() -> Result<Store, WgError> {
        let conn = Connection::open_in_memory().map_err(|err| db_error(&err))?;
        Store::configure(conn)
    }

    /// `_configure_connection`: the pragmas every connection gets.
    fn configure(conn: Connection) -> Result<Store, WgError> {
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(|err| db_error(&err))?;
        // `journal_mode` answers with the mode now in force, so it is a query.
        conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0))
            .map_err(|err| db_error(&err))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;")
            .map_err(|err| db_error(&err))?;
        Ok(Store { conn })
    }

    /// The raw connection, for reads outside a transaction and for setup (running DDL).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Runs `body` in a read transaction (a consistent snapshot); nothing is committed.
    pub fn read<T>(
        &mut self,
        body: impl FnOnce(&Connection) -> Result<T, WgError>,
    ) -> Result<T, WgError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|err| db_error(&err))?;
        body(&tx)
    }

    /// Runs `body` in a `BEGIN IMMEDIATE` transaction, so the write lock is taken up front. Commits
    /// when `body` returns `Ok`; any error rolls back every change `body` made.
    pub fn write<T>(
        &mut self,
        body: impl FnOnce(&Connection) -> Result<T, WgError>,
    ) -> Result<T, WgError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|err| db_error(&err))?;
        let value = body(&tx)?;
        tx.commit().map_err(|err| db_error(&err))?;
        Ok(value)
    }

    /// Applies a decision's writes and events in one write transaction. The caller performs the
    /// decision's effects afterwards.
    pub fn commit_decision<R>(&mut self, decision: &Decision<R>) -> Result<Applied, WgError> {
        self.write(|conn| apply_decision(conn, decision))
    }
}
