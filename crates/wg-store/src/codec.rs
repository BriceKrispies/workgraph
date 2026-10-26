//! Column codecs: how model values become SQLite values and back.
//!
//! These reproduce the Python `TypeDecorator`s of `persistence/types.py`:
//!
//! * ids are their canonical hyphenated text;
//! * timestamps are `UTCDateTime`, `YYYY-MM-DDTHH:MM:SS.ffffff+00:00` (O11);
//! * JSON columns are `JSONText`, sorted keys and compact separators (O10), via `wg-json`;
//! * booleans are `0` and `1`.

use rpds::VectorSync;
use rusqlite::Row;
use rusqlite::types::{Type, Value as Sql};
use serde_json::Value;
use wg_json::{DumpError, PyStr};
use wg_model::JsonObject;
use wg_types::{ErrorCode, Id, Timestamp, WgError, parse_id};

/// An id column.
pub fn id_sql(id: Id) -> Sql {
    Sql::Text(id.to_string())
}

/// A nullable id column.
pub fn opt_id_sql(id: Option<Id>) -> Sql {
    id.map_or(Sql::Null, id_sql)
}

/// A timestamp column.
pub fn ts_sql(at: Timestamp) -> Sql {
    Sql::Text(at.format_db())
}

/// A nullable timestamp column.
pub fn opt_ts_sql(at: Option<Timestamp>) -> Sql {
    at.map_or(Sql::Null, ts_sql)
}

/// A text column.
pub fn text_sql(text: &str) -> Sql {
    Sql::Text(text.to_owned())
}

/// A nullable text column.
pub fn opt_text_sql(text: &Option<String>) -> Sql {
    text.as_deref().map_or(Sql::Null, text_sql)
}

/// A nullable integer column.
pub fn opt_int_sql(value: Option<i64>) -> Sql {
    value.map_or(Sql::Null, Sql::Integer)
}

/// A boolean column.
pub fn bool_sql(value: bool) -> Sql {
    Sql::Integer(i64::from(value))
}

/// `serde_json` value to the `wg-json` value the O10 dialect writes.
pub fn to_json_value(value: &Value) -> wg_json::Value {
    match value {
        Value::Null => wg_json::Value::Null,
        Value::Bool(flag) => wg_json::Value::Bool(*flag),
        Value::Number(number) => number
            .as_i64()
            .map(|int| wg_json::Value::Int(i128::from(int)))
            .or_else(|| {
                number
                    .as_u64()
                    .map(|int| wg_json::Value::Int(i128::from(int)))
            })
            .unwrap_or_else(|| wg_json::Value::Float(number.as_f64().unwrap_or(f64::NAN))),
        Value::String(text) => wg_json::Value::Str(PyStr::from(text.as_str())),
        Value::Array(items) => wg_json::Value::List(items.iter().map(to_json_value).collect()),
        Value::Object(map) => wg_json::Value::Dict(
            map.iter()
                .map(|(key, item)| (PyStr::from(key.as_str()), to_json_value(item)))
                .collect(),
        ),
    }
}

fn dump_error(err: DumpError) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("cannot serialise JSON column: {err}"),
    )
}

/// The O10 text of a JSON value; `None` (SQL `NULL`) for JSON `null`.
pub fn json_text(value: &Value) -> Result<Option<String>, WgError> {
    wg_json::db_json(&to_json_value(value)).map_err(dump_error)
}

/// A NOT NULL JSON object column.
pub fn object_sql(object: &JsonObject) -> Result<Sql, WgError> {
    let text = json_text(&Value::Object(object.clone()))?;
    Ok(text.map_or(Sql::Null, Sql::Text))
}

/// A nullable JSON object column.
pub fn opt_object_sql(object: &Option<JsonObject>) -> Result<Sql, WgError> {
    object.as_ref().map_or(Ok(Sql::Null), object_sql)
}

/// A NOT NULL JSON list-of-strings column.
pub fn strings_sql(items: &VectorSync<String>) -> Result<Sql, WgError> {
    let list = Value::Array(items.iter().cloned().map(Value::String).collect());
    let text = json_text(&list)?;
    Ok(text.map_or(Sql::Null, Sql::Text))
}

/// Wraps a decode failure so rusqlite reports it as a column conversion error.
fn conversion(
    column: usize,
    err: impl std::error::Error + Send + Sync + 'static,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(err))
}

/// Named-column reader over a result row; decoders use it instead of positional indexes, so a
/// `SELECT` may list columns in any order.
pub struct Cols<'a, 'r> {
    row: &'a Row<'r>,
}

impl<'a, 'r> Cols<'a, 'r> {
    pub fn new(row: &'a Row<'r>) -> Self {
        Cols { row }
    }

    fn index(&self, name: &str) -> rusqlite::Result<usize> {
        self.row.as_ref().column_index(name)
    }

    pub fn text(&self, name: &str) -> rusqlite::Result<String> {
        self.row.get(name)
    }

    pub fn opt_text(&self, name: &str) -> rusqlite::Result<Option<String>> {
        self.row.get(name)
    }

    pub fn int(&self, name: &str) -> rusqlite::Result<i64> {
        self.row.get(name)
    }

    pub fn opt_int(&self, name: &str) -> rusqlite::Result<Option<i64>> {
        self.row.get(name)
    }

    pub fn flag(&self, name: &str) -> rusqlite::Result<bool> {
        Ok(self.int(name)? != 0)
    }

    pub fn id(&self, name: &str) -> rusqlite::Result<Id> {
        let text = self.text(name)?;
        parse_id(&text).map_err(|err| conversion(self.index(name).unwrap_or(0), err))
    }

    pub fn opt_id(&self, name: &str) -> rusqlite::Result<Option<Id>> {
        self.opt_text(name)?
            .map(|text| {
                parse_id(&text).map_err(|err| conversion(self.index(name).unwrap_or(0), err))
            })
            .transpose()
    }

    pub fn ts(&self, name: &str) -> rusqlite::Result<Timestamp> {
        let text = self.text(name)?;
        Timestamp::parse_iso(&text).map_err(|err| conversion(self.index(name).unwrap_or(0), err))
    }

    pub fn opt_ts(&self, name: &str) -> rusqlite::Result<Option<Timestamp>> {
        self.opt_text(name)?
            .map(|text| {
                Timestamp::parse_iso(&text)
                    .map_err(|err| conversion(self.index(name).unwrap_or(0), err))
            })
            .transpose()
    }

    /// A wire-string enum column.
    pub fn parsed<T: std::str::FromStr<Err = WgError>>(&self, name: &str) -> rusqlite::Result<T> {
        let text = self.text(name)?;
        text.parse()
            .map_err(|err| conversion(self.index(name).unwrap_or(0), err))
    }

    fn json(&self, text: &str, name: &str) -> rusqlite::Result<Value> {
        serde_json::from_str(text).map_err(|err| conversion(self.index(name).unwrap_or(0), err))
    }

    fn object_from(&self, value: Value, name: &str) -> rusqlite::Result<JsonObject> {
        match value {
            Value::Object(map) => Ok(map),
            other => Err(conversion(
                self.index(name).unwrap_or(0),
                WgError::new(
                    ErrorCode::WorkgraphError,
                    format!("column {name} holds {other}, not a JSON object"),
                ),
            )),
        }
    }

    pub fn object(&self, name: &str) -> rusqlite::Result<JsonObject> {
        let text = self.text(name)?;
        let value = self.json(&text, name)?;
        self.object_from(value, name)
    }

    pub fn opt_object(&self, name: &str) -> rusqlite::Result<Option<JsonObject>> {
        self.opt_text(name)?
            .map(|text| {
                let value = self.json(&text, name)?;
                self.object_from(value, name)
            })
            .transpose()
    }

    pub fn strings(&self, name: &str) -> rusqlite::Result<VectorSync<String>> {
        let text = self.text(name)?;
        match self.json(&text, name)? {
            Value::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Value::String(text) => Ok(text),
                    other => Err(conversion(
                        self.index(name).unwrap_or(0),
                        WgError::new(
                            ErrorCode::WorkgraphError,
                            format!("column {name} holds {other}, not a string"),
                        ),
                    )),
                })
                .collect(),
            other => Err(conversion(
                self.index(name).unwrap_or(0),
                WgError::new(
                    ErrorCode::WorkgraphError,
                    format!("column {name} holds {other}, not a JSON list"),
                ),
            )),
        }
    }
}

/// Maps a SQLite failure to the domain error the Python layer would raise for it: a constraint
/// violation (unique, foreign key, check, immutability trigger) is a `conflict`, a locked
/// database is `resource_busy`, anything else a `workgraph_error`.
pub fn db_error(err: &rusqlite::Error) -> WgError {
    let code = match err {
        rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
            rusqlite::ErrorCode::ConstraintViolation => ErrorCode::Conflict,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                ErrorCode::ResourceBusy
            }
            _ => ErrorCode::WorkgraphError,
        },
        _ => ErrorCode::WorkgraphError,
    };
    WgError::new(code, format!("database error: {err}"))
}
