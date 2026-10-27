//! `GET /api/v1/agents`: one entry per configured agent adapter. `capabilities` and `config` are the
//! provider's own dumps and are passed through as values.

use wg_json::Value;

use crate::wire_dto;

wire_dto! {
    /// One agent entry, keys in the order the registry builds it.
    pub struct AgentEntry<'a>("AgentEntry")
    dict agent(
        name: &'a str,
        adapter: &'a str,
        available: bool,
        detail: &'a str,
        capabilities: Value,
        config: Value
    ) {
        name: &'a str = name,
        adapter: &'a str = adapter,
        available: bool = available,
        detail: &'a str = detail,
        capabilities: Value = capabilities,
        config: Value = config,
    }
}

/// The endpoint's body: the list of entries.
pub fn agents(entries: Vec<Value>) -> Value {
    Value::List(entries)
}
