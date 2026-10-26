//! Helpers that fix the DTO conventions described in the crate documentation.

use serde_json::Value as Json;
use wg_json::{PyStr, Value};
use wg_types::{Id, Timestamp};

/// How a timestamp field is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    /// Pydantic's model form (`2026-10-25T13:56:00Z`), used by every `datetime` field of a model.
    ModelZ,
    /// The same `Z` text as [`TimeFormat::ModelZ`], written as a string so it prints the same in a
    /// plain dict. A route that returns a dict of datetimes gets it from FastAPI's encoder
    /// (`GET /jobs` prints `"run_at":"2026-10-26T04:55:43.038933Z"`).
    Z,
    /// Python `isoformat()` (`2026-10-25T13:56:00+00:00`), used where a handler stringifies the
    /// datetime itself.
    Isoformat,
}

/// A timestamp field in the given format.
pub fn timestamp(ts: Timestamp, format: TimeFormat) -> Value {
    match format {
        TimeFormat::ModelZ => Value::DateTime(ts),
        TimeFormat::Z => Value::Str(PyStr::from(ts.format_z())),
        TimeFormat::Isoformat => Value::Str(PyStr::from(ts.format_isoformat())),
    }
}

/// An optional timestamp field: `null` when absent.
pub fn optional_timestamp(ts: Option<Timestamp>, format: TimeFormat) -> Value {
    ts.map_or(Value::Null, |ts| timestamp(ts, format))
}

/// An id field: lowercase hyphenated text.
pub fn id(id: Id) -> Value {
    Value::Str(PyStr::from(id.hyphenated().to_string()))
}

/// A string field.
pub fn text(text: &str) -> Value {
    Value::Str(PyStr::from(text))
}

/// An optional string field: `null` when absent.
pub fn optional_text(text_value: Option<&str>) -> Value {
    text_value.map_or(Value::Null, text)
}

/// An optional id field: `null` when absent.
pub fn optional_id(id_value: Option<Id>) -> Value {
    id_value.map_or(Value::Null, id)
}

/// An optional JSON object field: `null` when absent.
pub fn optional_json_object(map: Option<&serde_json::Map<String, Json>>) -> Value {
    map.map_or(Value::Null, json_object)
}

/// A list-of-ids field.
pub fn ids(ids: impl IntoIterator<Item = Id>) -> Value {
    Value::List(ids.into_iter().map(id).collect())
}

/// A JSON value from a JSON column or a detail map; object key order is preserved.
pub fn json_value(value: &Json) -> Value {
    match value {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => n
            .as_i64()
            .map(|i| Value::Int(i128::from(i)))
            .or_else(|| n.as_u64().map(|u| Value::Int(i128::from(u))))
            .or_else(|| n.to_string().parse::<i128>().ok().map(Value::Int))
            .or_else(|| n.as_f64().map(Value::Float))
            .unwrap_or(Value::Null),
        Json::String(s) => text(s),
        Json::Array(items) => Value::List(items.iter().map(json_value).collect()),
        Json::Object(map) => json_object(map),
    }
}

/// A JSON object as an ordered dict value.
pub fn json_object(map: &serde_json::Map<String, Json>) -> Value {
    Value::Dict(
        map.iter()
            .map(|(key, value)| (PyStr::from(key.as_str()), json_value(value)))
            .collect(),
    )
}

/// A `dict[str, int]` field such as `tasks_by_status`: keys in the order given.
pub fn counts(pairs: &[(String, i64)]) -> Value {
    Value::Dict(
        pairs
            .iter()
            .map(|(key, count)| (PyStr::from(key.as_str()), Value::Int(i128::from(*count))))
            .collect(),
    )
}

/// An `int` field.
pub fn int(count: i64) -> Value {
    Value::Int(i128::from(count))
}

/// A model value: the class name and its fields in declaration order.
pub fn model(name: &str, fields: Vec<(&'static str, Value)>) -> Value {
    Value::Model {
        name: name.to_string(),
        fields: fields
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    }
}

/// An optional integer field: `null` when absent.
pub fn optional_int(value: Option<i64>) -> Value {
    value.map_or(Value::Null, int)
}

/// An optional float field: `null` when absent.
pub fn optional_float(value: Option<f64>) -> Value {
    value.map_or(Value::Null, Value::Float)
}

/// A list-of-text field.
pub fn texts<'a>(values: impl IntoIterator<Item = &'a str>) -> Value {
    Value::List(values.into_iter().map(text).collect())
}

/// A dict value, as a route handler returns a mapping: keys in the order given.
pub fn dict(fields: Vec<(&'static str, Value)>) -> Value {
    Value::Dict(
        fields
            .into_iter()
            .map(|(key, value)| (PyStr::from(key), value))
            .collect(),
    )
}

/// A `Page[T]` envelope (`items`, `total`, `limit`, `offset`, `next_offset`).
pub fn page(
    items: Vec<Value>,
    total: i64,
    limit: i64,
    offset: i64,
    next_offset: Option<i64>,
) -> Value {
    model(
        "Page",
        vec![
            ("items", Value::List(items)),
            ("total", int(total)),
            ("limit", int(limit)),
            ("offset", int(offset)),
            ("next_offset", optional_int(next_offset)),
        ],
    )
}
