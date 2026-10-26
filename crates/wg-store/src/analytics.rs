//! Source extraction for the analytics warehouse (SH-14): copies the tables and columns of
//! `TABLES` out of the database.
//!
//! The statements are built by `wg_storecodec::analytics_sql`; this module asks the database which
//! columns each table has, runs the copy and hands back the rows as values. Building the warehouse
//! from them is the analytics crate's job.

use wg_storecodec::Raw;
use wg_storecodec::analytics_sql::{self, TABLES};
use wg_types::WgError;

use crate::Session;

/// One table's copy: the columns read (in the order the rows hold them) and the rows, in rowid
/// order.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalyticsSource {
    pub table: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Raw>>,
}

impl Session<'_> {
    /// Copies one table of [`TABLES`]. A table that is not listed there, or has none of its wanted
    /// columns, fails with `workgraph_error`.
    pub fn analytics_source(&mut self, table: &str) -> Result<AnalyticsSource, WgError> {
        let wanted = analytics_sql::wanted_columns(table).ok_or_else(|| {
            wg_storecodec::db_error(
                wg_storecodec::DbFailure::Other,
                &format!("{table} is not an analytics source table"),
            )
        })?;
        let available = analytics_sql::column_names(&self.rows(&analytics_sql::table_info(table))?);
        let columns = analytics_sql::source_columns(Some(wanted), &available);
        let rows = self.rows(&analytics_sql::source_select(table, &columns)?)?;
        Ok(AnalyticsSource {
            table: table.to_owned(),
            columns,
            rows,
        })
    }

    /// Copies every table of [`TABLES`], in the order `TABLES` lists them.
    pub fn analytics_sources(&mut self) -> Result<Vec<AnalyticsSource>, WgError> {
        TABLES
            .iter()
            .map(|(table, _)| self.analytics_source(table))
            .collect()
    }
}
