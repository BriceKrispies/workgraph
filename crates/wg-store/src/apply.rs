//! Applying a decision's row writes and events.
//!
//! Every function takes the caller's open transaction (a `&Connection`, which a rusqlite
//! `Transaction` dereferences to); the store's [`crate::Store::commit_decision`] wraps them in one
//! `BEGIN IMMEDIATE` transaction.

use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension, params_from_iter};
use wg_model::{Decision, RowKey, Table, Write};
use wg_types::{ErrorCode, WgError};

use crate::codec::db_error;
use crate::events::append_event;
use crate::rows::{TableRow, columns, key_values, primary_key, row_values};

/// What one applied decision did to the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The `seq` of each appended event, in decision order.
    pub event_seqs: Vec<i64>,
}

/// Quotes an identifier (`key` is a keyword-adjacent column name).
fn quote(name: &str) -> String {
    format!("\"{name}\"")
}

/// `col1 = ?1 AND col2 = ?2`, numbering the placeholders from `first`.
fn key_clause(table: Table, first: usize) -> String {
    primary_key(table)
        .iter()
        .enumerate()
        .map(|(offset, column)| format!("{} = ?{}", quote(column), first + offset))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn insert_sql(table: Table) -> String {
    let cols = columns(table);
    let names: Vec<String> = cols.iter().map(|c| quote(c)).collect();
    let marks: Vec<String> = (1..=cols.len()).map(|n| format!("?{n}")).collect();
    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote(table.name()),
        names.join(", "),
        marks.join(", ")
    )
}

/// Applies one write. An insert clashing with a primary key or unique constraint fails with
/// `conflict`; an update that finds no row fails with `not_found`, or `version_conflict` when it
/// was guarded and the row exists at another version; a delete of an absent row does nothing.
pub fn apply_write(conn: &Connection, write: &Write) -> Result<(), WgError> {
    match write {
        Write::Insert(row) => {
            let values = row_values(row)?;
            let mut statement = conn
                .prepare_cached(&insert_sql(row.table()))
                .map_err(|err| db_error(&err))?;
            statement
                .execute(params_from_iter(values))
                .map_err(|err| db_error(&err))?;
            Ok(())
        }
        Write::Update {
            row,
            expected_version,
        } => update_row(conn, row, *expected_version),
        Write::Delete(key) => {
            let table = key.table();
            let sql = format!(
                "DELETE FROM {} WHERE {}",
                quote(table.name()),
                key_clause(table, 1)
            );
            let mut statement = conn.prepare_cached(&sql).map_err(|err| db_error(&err))?;
            statement
                .execute(params_from_iter(key_values(key)))
                .map_err(|err| db_error(&err))?;
            Ok(())
        }
    }
}

fn update_row(
    conn: &Connection,
    row: &wg_model::Row,
    expected_version: Option<i64>,
) -> Result<(), WgError> {
    let table = row.table();
    let key = row.key();
    let cols = columns(table);
    let pk = primary_key(table);
    let mut values = row_values(row)?;
    let key_params = key_values(&key);

    let assignments: Vec<String> = cols
        .iter()
        .enumerate()
        .filter(|(_, column)| !pk.contains(column))
        .map(|(index, column)| format!("{} = ?{}", quote(column), index + 1))
        .collect();
    // The key columns' positions in the full parameter list, so WHERE reuses the row's values.
    let where_parts: Vec<String> = pk
        .iter()
        .map(|column| {
            let position = cols.iter().position(|c| c == column).map_or(0, |p| p + 1);
            format!("{} = ?{}", quote(column), position)
        })
        .collect();
    let mut condition = where_parts.join(" AND ");
    if let Some(version) = expected_version {
        values.push(Sql::Integer(version));
        condition.push_str(&format!(" AND \"version\" = ?{}", values.len()));
    }

    let changed = if assignments.is_empty() {
        // Every column is part of the key: the "replacement" is the row itself.
        let sql = format!(
            "SELECT COUNT(*) FROM {} WHERE {}",
            quote(table.name()),
            key_clause(table, 1)
        );
        let count: i64 = conn
            .query_row(&sql, params_from_iter(key_params.clone()), |r| r.get(0))
            .map_err(|err| db_error(&err))?;
        usize::try_from(count).unwrap_or(0)
    } else {
        let sql = format!(
            "UPDATE {} SET {} WHERE {}",
            quote(table.name()),
            assignments.join(", "),
            condition
        );
        let mut statement = conn.prepare_cached(&sql).map_err(|err| db_error(&err))?;
        statement
            .execute(params_from_iter(values))
            .map_err(|err| db_error(&err))?
    };
    if changed > 0 {
        return Ok(());
    }
    Err(update_failure(conn, table, &key, expected_version))
}

/// Explains an update that matched no row.
fn update_failure(
    conn: &Connection,
    table: Table,
    key: &RowKey,
    expected_version: Option<i64>,
) -> WgError {
    if let Some(expected) = expected_version {
        let sql = format!(
            "SELECT \"version\" FROM {} WHERE {}",
            quote(table.name()),
            key_clause(table, 1)
        );
        let actual: Result<Option<i64>, _> = conn
            .query_row(&sql, params_from_iter(key_values(key)), |r| r.get(0))
            .optional();
        if let Ok(Some(actual)) = actual {
            return WgError::new(
                ErrorCode::VersionConflict,
                format!(
                    "{} row was modified concurrently (expected version {expected}, found \
                     {actual}); re-read and retry",
                    table.name()
                ),
            )
            .with_detail("expected", expected.into())
            .with_detail("actual", actual.into());
        }
    }
    WgError::new(
        ErrorCode::NotFound,
        format!("no {} row to update", table.name()),
    )
}

/// Applies a decision's writes in order, then appends its events in order. Effects are not the
/// store's: the runtime performs them after the transaction commits.
pub fn apply_decision<R>(conn: &Connection, decision: &Decision<R>) -> Result<Applied, WgError> {
    for write in &decision.writes {
        apply_write(conn, write)?;
    }
    let mut event_seqs = Vec::new();
    for event in &decision.events {
        event_seqs.push(append_event(conn, event)?);
    }
    Ok(Applied { event_seqs })
}

/// Reads the row with primary key `key`, if it exists.
pub fn fetch<T: TableRow>(conn: &Connection, key: &RowKey) -> Result<Option<T>, WgError> {
    if key.table() != T::TABLE {
        return Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!(
                "key for {} used to read {}",
                key.table().name(),
                T::TABLE.name()
            ),
        ));
    }
    let names: Vec<String> = columns(T::TABLE).iter().map(|c| quote(c)).collect();
    let sql = format!(
        "SELECT {} FROM {} WHERE {}",
        names.join(", "),
        quote(T::TABLE.name()),
        key_clause(T::TABLE, 1)
    );
    conn.query_row(&sql, params_from_iter(key_values(key)), T::from_sql_row)
        .optional()
        .map_err(|err| db_error(&err))
}
