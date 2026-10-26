//! Source extraction for the analytics warehouse (SH-14): copies the tables and columns of
//! `TABLES` out of the database.
//!
//! The statements are built by [`crate::spec::sql`]; this module asks the database which columns
//! each table has, runs the copy and hands back the rows as values. Building the warehouse from
//! them is the analytics crate's job (`wg-analytics`).

use wg_store::Session;
use wg_types::WgError;

use crate::spec::dto::AnalyticsSource;
use crate::spec::sql::{self, TABLES};

/// Copies one table of [`TABLES`]. A table that is not listed there, or has none of its wanted
/// columns, fails with `workgraph_error`.
pub fn analytics_source(
    session: &mut Session<'_>,
    table: &str,
) -> Result<AnalyticsSource, WgError> {
    let wanted = sql::wanted_columns(table).ok_or_else(|| {
        wg_storecodec::db_error(
            wg_storecodec::DbFailure::Other,
            &format!("{table} is not an analytics source table"),
        )
    })?;
    let available = sql::column_names(&session.rows(&sql::table_info(table))?);
    let columns = sql::source_columns(Some(wanted), &available);
    let rows = session.rows(&sql::source_select(table, &columns)?)?;
    Ok(AnalyticsSource {
        table: table.to_owned(),
        columns,
        rows,
    })
}

/// Copies every table of [`TABLES`], in the order `TABLES` lists them.
pub fn analytics_sources(session: &mut Session<'_>) -> Result<Vec<AnalyticsSource>, WgError> {
    TABLES
        .iter()
        .map(|(table, _)| analytics_source(session, table))
        .collect()
}
