//! The unauthenticated health probe and the `meta` document. Both are dicts the handlers build
//! from the version and the configuration, so they take those values as arguments.

use wg_json::{PyStr, Value};

use crate::convention::text;

/// The keys of `GET /api/v1/health`.
pub const HEALTH_FIELDS: [&str; 2] = ["status", "version"];

/// The keys of `GET /api/v1/meta`.
pub const META_FIELDS: [&str; 6] = [
    "version",
    "default_workspace",
    "execution_enabled",
    "auto_schedule",
    "agents",
    "llm",
];

/// The keys of the `llm` object of `meta`.
pub const META_LLM_FIELDS: [&str; 2] = ["classification", "embeddings"];

/// `GET /api/v1/health`: always `ok` while the process answers.
pub fn health(version: &str) -> Value {
    dict(vec![("status", text("ok")), ("version", text(version))])
}

/// `GET /api/v1/meta`. `agents` are the configured agent names; `classification` and
/// `embeddings` are the dumped LLM sections of the configuration.
pub fn meta(
    version: &str,
    default_workspace: &str,
    execution_enabled: bool,
    auto_schedule: bool,
    agents: &[String],
    classification: Value,
    embeddings: Value,
) -> Value {
    dict(vec![
        ("version", text(version)),
        ("default_workspace", text(default_workspace)),
        ("execution_enabled", Value::Bool(execution_enabled)),
        ("auto_schedule", Value::Bool(auto_schedule)),
        (
            "agents",
            Value::List(agents.iter().map(|name| text(name)).collect()),
        ),
        (
            "llm",
            dict(vec![
                ("classification", classification),
                ("embeddings", embeddings),
            ]),
        ),
    ])
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
