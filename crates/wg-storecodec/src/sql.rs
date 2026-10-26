//! SQL text and parameters as values: the statements that apply a [`Write`] or read a row by
//! key. The store only binds and runs them.

use wg_model::{Row, RowKey, Table, Write};
use wg_types::{ErrorCode, WgError};

use crate::codec::Raw;
use crate::rows::{columns, key_values, primary_key, row_values};

/// One statement: text with `?N` placeholders and the values to bind, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub sql: String,
    pub params: Vec<Raw>,
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

fn insert_statement(row: &Row) -> Result<Statement, WgError> {
    let table = row.table();
    let cols = columns(table);
    let names: Vec<String> = cols.iter().map(|c| quote(c)).collect();
    let marks: Vec<String> = (1..=cols.len()).map(|n| format!("?{n}")).collect();
    Ok(Statement {
        sql: format!(
            "INSERT INTO {} ({}) VALUES ({})",
            quote(table.name()),
            names.join(", "),
            marks.join(", ")
        ),
        params: row_values(row)?,
    })
}

/// Replaces every column of the row with the same primary key. A table whose columns are all key
/// columns gets a self-assignment, so the statement still reports whether the row exists.
fn update_statement(row: &Row, expected_version: Option<i64>) -> Result<Statement, WgError> {
    let table = row.table();
    let cols = columns(table);
    let pk = primary_key(table);
    let params = row_values(row)?;
    let position = |column: &str| cols.iter().position(|c| *c == column).map_or(0, |p| p + 1);
    let mut assignments: Vec<String> = cols
        .iter()
        .filter(|column| !pk.contains(column))
        .map(|column| format!("{} = ?{}", quote(column), position(column)))
        .collect();
    if assignments.is_empty() {
        assignments = pk
            .first()
            .map(|column| format!("{} = {}", quote(column), quote(column)))
            .into_iter()
            .collect();
    }
    let mut condition = pk
        .iter()
        .map(|column| format!("{} = ?{}", quote(column), position(column)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let params = match expected_version {
        Some(version) => {
            condition.push_str(&format!(" AND \"version\" = ?{}", params.len() + 1));
            params
                .into_iter()
                .chain(core::iter::once(Raw::Int(version)))
                .collect()
        }
        None => params,
    };
    Ok(Statement {
        sql: format!(
            "UPDATE {} SET {} WHERE {}",
            quote(table.name()),
            assignments.join(", "),
            condition
        ),
        params,
    })
}

fn delete_statement(key: &RowKey) -> Statement {
    let table = key.table();
    Statement {
        sql: format!(
            "DELETE FROM {} WHERE {}",
            quote(table.name()),
            key_clause(table, 1)
        ),
        params: key_values(key),
    }
}

/// The statement for one [`Write`]. An insert that clashes with a key or unique constraint fails;
/// an update that matches no row changes nothing (see [`update_failure`]); a delete of an absent
/// row does nothing.
pub fn write_statement(write: &Write) -> Result<Statement, WgError> {
    match write {
        Write::Insert(row) => insert_statement(row),
        Write::Update {
            row,
            expected_version,
        } => update_statement(row, *expected_version),
        Write::Delete(key) => Ok(delete_statement(key)),
    }
}

/// The stored version of a versioned row, to explain a failed guarded update.
pub fn version_probe(key: &RowKey) -> Statement {
    let table = key.table();
    Statement {
        sql: format!(
            "SELECT \"version\" FROM {} WHERE {}",
            quote(table.name()),
            key_clause(table, 1)
        ),
        params: key_values(key),
    }
}

/// Explains an update that matched no row: `actual` is the version [`version_probe`] found, if
/// the row exists. A guarded update on an existing row is a `version_conflict`; otherwise the
/// row is missing and the error is `not_found`.
pub fn update_failure(table: Table, expected: Option<i64>, actual: Option<i64>) -> WgError {
    match (expected, actual) {
        (Some(expected), Some(actual)) => WgError::new(
            ErrorCode::VersionConflict,
            format!(
                "{} row was modified concurrently (expected version {expected}, found \
                 {actual}); re-read and retry",
                table.name()
            ),
        )
        .with_detail("expected", expected.into())
        .with_detail("actual", actual.into()),
        _ => WgError::new(
            ErrorCode::NotFound,
            format!("no {} row to update", table.name()),
        ),
    }
}

/// Reads the row with primary key `key`, selecting [`columns`] in order.
pub fn select_by_key(key: &RowKey) -> Statement {
    let table = key.table();
    let names: Vec<String> = columns(table).iter().map(|c| quote(c)).collect();
    Statement {
        sql: format!(
            "SELECT {} FROM {} WHERE {}",
            names.join(", "),
            quote(table.name()),
            key_clause(table, 1)
        ),
        params: key_values(key),
    }
}
