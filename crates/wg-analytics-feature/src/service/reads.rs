//! Source extraction for the analytics warehouse (SH-14): copies the tables and columns of
//! `TABLES` out of the database.
//!
//! The statements are built by [`crate::spec::sql`]; this module asks the database which columns
//! each table has, runs the copy and hands back the rows as values. Building the warehouse from
//! them is the analytics crate's job (`wg-analytics`); [`analytics_input`] turns the copies into
//! its typed source rows.

use wg_analytics::source::AnalyticsInput;
use wg_store::Session;
use wg_types::WgError;

use crate::spec::dto::AnalyticsSource;
use crate::spec::load;
use crate::spec::sql::{self, EVENT_COLUMNS, TABLES};

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

/// The warehouse's copy of the `events` rows of [`sql::EVENT_TYPES`].
pub fn analytics_events(session: &mut Session<'_>) -> Result<AnalyticsSource, WgError> {
    let rows = session.rows(&sql::events_select())?;
    Ok(AnalyticsSource {
        table: "events".to_owned(),
        columns: EVENT_COLUMNS
            .iter()
            .map(|column| (*column).to_owned())
            .collect(),
        rows,
    })
}

/// The copy of `subagent_runs` (not in [`TABLES`]: Python reads it through its ORM).
pub fn subagent_runs_source(session: &mut Session<'_>) -> Result<AnalyticsSource, WgError> {
    let rows = session.rows(&sql::subagent_runs_select())?;
    Ok(AnalyticsSource {
        table: "subagent_runs".to_owned(),
        columns: sql::SUBAGENT_RUN_COLUMNS
            .iter()
            .map(|column| (*column).to_owned())
            .collect(),
        rows,
    })
}

/// Every source table of the warehouse, as the typed rows of `wg-analytics`.
pub fn analytics_input(session: &mut Session<'_>) -> Result<AnalyticsInput, WgError> {
    let mut sources = analytics_sources(session)?;
    sources.push(analytics_events(session)?);
    sources.push(subagent_runs_source(session)?);
    Ok(load::analytics_input(&sources))
}
