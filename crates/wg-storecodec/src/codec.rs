//! Column codecs: how model values become database values and back.
//!
//! These reproduce the Python `TypeDecorator`s of `persistence/types.py`:
//!
//! * ids are their canonical hyphenated text;
//! * timestamps are `UTCDateTime`, `YYYY-MM-DDTHH:MM:SS.ffffff+00:00` (O11);
//! * JSON columns are `JSONText`, sorted keys and compact separators (O10), via `wg-json`;
//! * booleans are `0` and `1`.
//!
//! A database value is a [`Raw`]: the store converts between it and its driver's own value type.

use rpds::VectorSync;
use serde_json::Value;
use wg_json::{DumpError, PyStr};
use wg_model::JsonObject;
use wg_model::entity::{DecisionOption, Real};
use wg_types::{ErrorCode, Id, Timestamp, WgError, parse_id};

/// One database value, as the three storage classes the schema uses.
#[derive(Debug, Clone, PartialEq)]
pub enum Raw {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// An id column.
pub fn id_sql(id: Id) -> Raw {
    Raw::Text(id.to_string())
}

/// A nullable id column.
pub fn opt_id_sql(id: Option<Id>) -> Raw {
    id.map_or(Raw::Null, id_sql)
}

/// A timestamp column.
pub fn ts_sql(at: Timestamp) -> Raw {
    Raw::Text(at.format_db())
}

/// A nullable timestamp column.
pub fn opt_ts_sql(at: Option<Timestamp>) -> Raw {
    at.map_or(Raw::Null, ts_sql)
}

/// A text column.
pub fn text_sql(text: &str) -> Raw {
    Raw::Text(text.to_owned())
}

/// A nullable text column.
pub fn opt_text_sql(text: &Option<String>) -> Raw {
    text.as_deref().map_or(Raw::Null, text_sql)
}

/// A nullable integer column.
pub fn opt_int_sql(value: Option<i64>) -> Raw {
    value.map_or(Raw::Null, Raw::Int)
}

/// A boolean column.
pub fn bool_sql(value: bool) -> Raw {
    Raw::Int(i64::from(value))
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
pub fn object_sql(object: &JsonObject) -> Result<Raw, WgError> {
    let text = json_text(&Value::Object(object.clone()))?;
    Ok(text.map_or(Raw::Null, Raw::Text))
}

/// A nullable JSON object column.
pub fn opt_object_sql(object: &Option<JsonObject>) -> Result<Raw, WgError> {
    object.as_ref().map_or(Ok(Raw::Null), object_sql)
}

/// A NOT NULL JSON list-of-strings column.
pub fn strings_sql(items: &VectorSync<String>) -> Result<Raw, WgError> {
    let list = Value::Array(items.iter().cloned().map(Value::String).collect());
    let text = json_text(&list)?;
    Ok(text.map_or(Raw::Null, Raw::Text))
}

/// A nullable `FLOAT` column.
pub fn opt_real_sql(value: Option<Real>) -> Raw {
    value.map_or(Raw::Null, real_sql)
}

/// A `FLOAT` column.
pub fn real_sql(value: Real) -> Raw {
    Raw::Real(value.0)
}

/// A `BLOB` column.
pub fn blob_sql(bytes: &VectorSync<u8>) -> Raw {
    Raw::Blob(bytes.iter().copied().collect())
}

/// A NOT NULL JSON list of objects.
pub fn objects_sql(items: &VectorSync<JsonObject>) -> Result<Raw, WgError> {
    let list = Value::Array(items.iter().cloned().map(Value::Object).collect());
    let text = json_text(&list)?;
    Ok(text.map_or(Raw::Null, Raw::Text))
}

/// A NOT NULL JSON list of decision options, each `{id, label, detail, recommended}`.
pub fn decision_options_sql<'a>(
    options: impl IntoIterator<Item = &'a DecisionOption>,
) -> Result<Raw, WgError> {
    let list = Value::Array(
        options
            .into_iter()
            .map(|option| {
                let mut object = JsonObject::new();
                object.insert("id".to_owned(), Value::String(option.id.clone()));
                object.insert("label".to_owned(), Value::String(option.label.clone()));
                object.insert("detail".to_owned(), Value::String(option.detail.clone()));
                object.insert("recommended".to_owned(), Value::Bool(option.recommended));
                Value::Object(object)
            })
            .collect(),
    );
    let text = json_text(&list)?;
    Ok(text.map_or(Raw::Null, Raw::Text))
}

/// A NOT NULL JSON list of arbitrary values.
pub fn values_sql(items: &VectorSync<Value>) -> Result<Raw, WgError> {
    let list = Value::Array(items.iter().cloned().collect());
    let text = json_text(&list)?;
    Ok(text.map_or(Raw::Null, Raw::Text))
}

fn corrupt(column: &str, what: &str) -> WgError {
    WgError::new(
        ErrorCode::WorkgraphError,
        format!("column {column}: {what}"),
    )
}

/// Named-column reader over the values of one result row; `names` is the column order the row
/// was selected in.
pub struct Cols<'a> {
    names: &'static [&'static str],
    values: &'a [Raw],
}

impl<'a> Cols<'a> {
    pub fn new(names: &'static [&'static str], values: &'a [Raw]) -> Self {
        Cols { names, values }
    }

    fn raw(&self, name: &str) -> Result<&Raw, WgError> {
        self.names
            .iter()
            .position(|candidate| *candidate == name)
            .and_then(|index| self.values.get(index))
            .ok_or_else(|| corrupt(name, "not in the result row"))
    }

    pub fn opt_text(&self, name: &str) -> Result<Option<String>, WgError> {
        match self.raw(name)? {
            Raw::Null => Ok(None),
            Raw::Text(text) => Ok(Some(text.clone())),
            _ => Err(corrupt(name, "expected text")),
        }
    }

    pub fn text(&self, name: &str) -> Result<String, WgError> {
        self.opt_text(name)?
            .ok_or_else(|| corrupt(name, "unexpected NULL"))
    }

    pub fn opt_int(&self, name: &str) -> Result<Option<i64>, WgError> {
        match self.raw(name)? {
            Raw::Null => Ok(None),
            Raw::Int(value) => Ok(Some(*value)),
            _ => Err(corrupt(name, "expected an integer")),
        }
    }

    pub fn int(&self, name: &str) -> Result<i64, WgError> {
        self.opt_int(name)?
            .ok_or_else(|| corrupt(name, "unexpected NULL"))
    }

    pub fn opt_real(&self, name: &str) -> Result<Option<Real>, WgError> {
        match self.raw(name)? {
            Raw::Null => Ok(None),
            Raw::Real(value) => Ok(Some(Real(*value))),
            Raw::Int(value) => Ok(Some(Real(*value as f64))),
            _ => Err(corrupt(name, "expected a float")),
        }
    }

    pub fn real(&self, name: &str) -> Result<Real, WgError> {
        self.opt_real(name)?
            .ok_or_else(|| corrupt(name, "unexpected NULL"))
    }

    pub fn blob(&self, name: &str) -> Result<VectorSync<u8>, WgError> {
        match self.raw(name)? {
            Raw::Blob(bytes) => Ok(bytes.iter().copied().collect()),
            _ => Err(corrupt(name, "expected a blob")),
        }
    }

    pub fn flag(&self, name: &str) -> Result<bool, WgError> {
        Ok(self.int(name)? != 0)
    }

    pub fn id(&self, name: &str) -> Result<Id, WgError> {
        parse_id(&self.text(name)?)
    }

    pub fn opt_id(&self, name: &str) -> Result<Option<Id>, WgError> {
        self.opt_text(name)?.map(|text| parse_id(&text)).transpose()
    }

    pub fn ts(&self, name: &str) -> Result<Timestamp, WgError> {
        Timestamp::parse_iso(&self.text(name)?)
    }

    pub fn opt_ts(&self, name: &str) -> Result<Option<Timestamp>, WgError> {
        self.opt_text(name)?
            .map(|text| Timestamp::parse_iso(&text))
            .transpose()
    }

    /// A wire-string enum column.
    pub fn parsed<T: std::str::FromStr<Err = WgError>>(&self, name: &str) -> Result<T, WgError> {
        self.text(name)?.parse()
    }

    fn json(&self, text: &str, name: &str) -> Result<Value, WgError> {
        serde_json::from_str(text).map_err(|err| corrupt(name, &format!("invalid JSON: {err}")))
    }

    fn object_from(value: Value, name: &str) -> Result<JsonObject, WgError> {
        match value {
            Value::Object(map) => Ok(map),
            other => Err(corrupt(name, &format!("{other} is not a JSON object"))),
        }
    }

    pub fn object(&self, name: &str) -> Result<JsonObject, WgError> {
        Self::object_from(self.json(&self.text(name)?, name)?, name)
    }

    pub fn opt_object(&self, name: &str) -> Result<Option<JsonObject>, WgError> {
        self.opt_text(name)?
            .map(|text| Self::object_from(self.json(&text, name)?, name))
            .transpose()
    }

    fn list(&self, name: &str) -> Result<Vec<Value>, WgError> {
        match self.json(&self.text(name)?, name)? {
            Value::Array(items) => Ok(items),
            other => Err(corrupt(name, &format!("{other} is not a JSON list"))),
        }
    }

    pub fn values(&self, name: &str) -> Result<VectorSync<Value>, WgError> {
        Ok(self.list(name)?.into_iter().collect())
    }

    pub fn objects(&self, name: &str) -> Result<VectorSync<JsonObject>, WgError> {
        self.list(name)?
            .into_iter()
            .map(|item| Self::object_from(item, name))
            .collect()
    }

    /// A NOT NULL JSON list of decision options.
    pub fn decision_options(&self, name: &str) -> Result<VectorSync<DecisionOption>, WgError> {
        self.list(name)?
            .into_iter()
            .map(|item| {
                let Value::Object(object) = item else {
                    return Err(corrupt(name, "a decision option is not a JSON object"));
                };
                let text = |key: &str| match object.get(key) {
                    Some(Value::String(text)) => Ok(text.clone()),
                    _ => Err(corrupt(name, &format!("option {key} is not text"))),
                };
                Ok(DecisionOption {
                    id: text("id")?,
                    label: text("label")?,
                    detail: text("detail")?,
                    recommended: match object.get("recommended") {
                        Some(Value::Bool(flag)) => *flag,
                        _ => return Err(corrupt(name, "option recommended is not a boolean")),
                    },
                })
            })
            .collect()
    }

    pub fn strings(&self, name: &str) -> Result<VectorSync<String>, WgError> {
        match self.json(&self.text(name)?, name)? {
            Value::Array(items) => items
                .into_iter()
                .map(|item| match item {
                    Value::String(text) => Ok(text),
                    other => Err(corrupt(name, &format!("{other} is not a string"))),
                })
                .collect(),
            other => Err(corrupt(name, &format!("{other} is not a JSON list"))),
        }
    }
}

/// How a database failure is classed; the store classifies its driver's error and this crate
/// chooses the domain code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbFailure {
    /// A unique, foreign-key, check or immutability-trigger violation.
    Constraint,
    /// The database is locked or busy.
    Busy,
    /// Anything else.
    Other,
}

/// The domain error for a database failure: a constraint violation is a `conflict`, a locked
/// database is `resource_busy`, anything else a `workgraph_error`.
pub fn db_error(kind: DbFailure, message: &str) -> WgError {
    let code = match kind {
        DbFailure::Constraint => ErrorCode::Conflict,
        DbFailure::Busy => ErrorCode::ResourceBusy,
        DbFailure::Other => ErrorCode::WorkgraphError,
    };
    WgError::new(code, format!("database error: {message}"))
}
