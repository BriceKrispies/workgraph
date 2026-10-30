//! wg-rows: the store's values, with no database in sight.
//!
//! Everything about the database that does not need a database: how model values become column
//! values and back ([`codec`], [`rows`]), the SQL text and parameters that apply a write or read a
//! row ([`sql`]), and the `events` log's rows, queries and upcasting ([`events`], [`query`]). The
//! statements are values ([`sql::Statement`]); `wg-store` binds them to its driver and runs them.

#[macro_use]
mod rowmap;
pub mod acceptance_sql;
pub mod codec;
pub mod events;
pub mod migrations;
pub mod query;
pub mod queue_sql;
pub mod repo_sql;
pub mod rows;
pub mod run_sql;
pub mod sql;
pub mod stack_sql;
pub mod task_sql;
pub mod task_world_sql;

/// Paths the exported macros expand to.
#[doc(hidden)]
pub mod __private {
    pub use wg_types::{TaskKey, WgError};
}

pub use codec::{Cols, DbFailure, Raw, db_error};
pub use events::{EVENT_COLUMNS, EventQuery, EventRecord, append_statement, decode_event};
pub use query::latest_seq_statement;
pub use rows::{TableRow, columns, key_values, primary_key, row_values};
pub use sql::{Statement, select_by_key, update_failure, version_probe, write_statement};
