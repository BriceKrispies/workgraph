//! wg-storecodec: the store's pure half.
//!
//! Everything about the database that does not need a database: how model values become column
//! values and back ([`codec`], [`rows`]), the SQL text and parameters that apply a write or read a
//! row ([`sql`]), and the `events` log's rows, queries and upcasting ([`events`], [`query`]). The
//! statements are values ([`sql::Statement`]); `wg-store` binds them to its driver and runs them.

pub mod classify_sql;
pub mod codec;
pub mod events;
pub mod migrations;
pub mod query;
pub mod repo_sql;
pub mod rows;
mod rows_more;
pub mod search_sql;
pub mod sql;
pub mod task_sql;

pub use codec::{Cols, DbFailure, Raw, db_error};
pub use events::{EVENT_COLUMNS, EventQuery, EventRecord, append_statement, decode_event};
pub use query::latest_seq_statement;
pub use rows::{TableRow, columns, key_values, primary_key, row_values};
pub use sql::{Statement, select_by_key, update_failure, version_probe, write_statement};
