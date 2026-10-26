//! `GET /api/v1/llm/health`: a dict keyed by provider name, each value `{available, detail}`.

use wg_json::{PyStr, Value};

use crate::convention::text;

/// The keys of one provider entry.
pub const ENTRY_FIELDS: [&str; 2] = ["available", "detail"];

/// The health report: one entry per provider, in the order given.
pub fn health<'a>(providers: impl IntoIterator<Item = (&'a str, bool, &'a str)>) -> Value {
    Value::Dict(
        providers
            .into_iter()
            .map(|(name, available, detail)| {
                (
                    PyStr::from(name),
                    dict(vec![
                        ("available", Value::Bool(available)),
                        ("detail", text(detail)),
                    ]),
                )
            })
            .collect(),
    )
}

/// A dict value with its keys in the given order.
fn dict(fields: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        fields
            .into_iter()
            .map(|(key, value)| (PyStr::from(key), value))
            .collect(),
    )
}
