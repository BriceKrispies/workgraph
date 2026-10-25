//! Parsing a TOML document into the JSON-shaped value the model validates.
//!
//! Key order is kept (the `toml` crate's `preserve_order` feature), so extra keys are reported in
//! document order as Pydantic reports them.

use serde_json::{Map, Value};

use crate::error::ConfigError;

/// Parses a configuration document. Invalid TOML is [`ConfigError::InvalidToml`].
pub fn parse_document(text: &str) -> Result<Map<String, Value>, ConfigError> {
    let table: toml::Table =
        toml::from_str(text).map_err(|error| ConfigError::InvalidToml(error.to_string()))?;
    Ok(table
        .into_iter()
        .map(|(key, value)| (key, from_toml(value)))
        .collect())
}

fn from_toml(value: toml::Value) -> Value {
    match value {
        toml::Value::String(text) => Value::String(text),
        toml::Value::Integer(int) => Value::from(int),
        // A non-finite float has no JSON form; it becomes null and fails validation as a number.
        toml::Value::Float(float) => Value::from(float),
        toml::Value::Boolean(flag) => Value::Bool(flag),
        toml::Value::Datetime(datetime) => Value::String(datetime.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(from_toml).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, value)| (key, from_toml(value)))
                .collect(),
        ),
    }
}
