//! The error shape: Pydantic's `errors(include_url=False)` items.

use serde_json::{Map, Value};

/// One step of an error location: a field or dict key, or a list index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loc {
    Key(String),
    Index(usize),
}

impl Loc {
    /// The JSON form of the location element.
    pub fn to_json(&self) -> Value {
        match self {
            Loc::Key(key) => Value::String(key.clone()),
            Loc::Index(index) => Value::from(*index),
        }
    }
}

/// A Pydantic validation error.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidationError {
    /// Pydantic's error type, such as `missing` or `string_too_short`.
    pub kind: String,
    pub loc: Vec<Loc>,
    pub msg: String,
    /// The offending input.
    pub input: Value,
    /// The context object, when Pydantic gives one.
    pub ctx: Option<Map<String, Value>>,
}

impl ValidationError {
    /// The error as the JSON object `{type, loc, msg, input, ctx?}`.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("type".into(), Value::String(self.kind.clone()));
        out.insert(
            "loc".into(),
            Value::Array(self.loc.iter().map(Loc::to_json).collect()),
        );
        out.insert("msg".into(), Value::String(self.msg.clone()));
        out.insert("input".into(), self.input.clone());
        if let Some(ctx) = &self.ctx {
            out.insert("ctx".into(), Value::Object(ctx.clone()));
        }
        Value::Object(out)
    }
}

pub(crate) fn error(
    kind: &str,
    msg: impl Into<String>,
    loc: &[Loc],
    input: &Value,
    ctx: Option<Map<String, Value>>,
) -> ValidationError {
    ValidationError {
        kind: kind.to_string(),
        loc: loc.to_vec(),
        msg: msg.into(),
        input: input.clone(),
        ctx,
    }
}

/// A context object from key and value pairs.
pub(crate) fn ctx<const N: usize>(pairs: [(&str, Value); N]) -> Option<Map<String, Value>> {
    Some(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}
