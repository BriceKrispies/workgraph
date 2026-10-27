//! `GET /api/v1/agents`: one entry per configured agent adapter. `capabilities` and `config` are the
//! provider's own dumps and are passed through as values.

use wg_json::{Value, dict};

use crate::convention::text;

/// The keys of one agent entry, in the order the registry builds it.
pub const FIELDS: [&str; 6] = [
    "name",
    "adapter",
    "available",
    "detail",
    "capabilities",
    "config",
];

/// One agent entry.
pub fn agent(
    name: &str,
    adapter: &str,
    available: bool,
    detail: &str,
    capabilities: Value,
    config: Value,
) -> Value {
    dict!({
        "name": text(name),
        "adapter": text(adapter),
        "available": Value::Bool(available),
        "detail": text(detail),
        "capabilities": capabilities,
        "config": config,
    })
}

/// The endpoint's body: the list of entries.
pub fn agents(entries: Vec<Value>) -> Value {
    Value::List(entries)
}
