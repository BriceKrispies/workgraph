//! `GET /api/v1/llm/health`: a dict keyed by provider name, each value `{available, detail}`.

use wg_json::{PyStr, Value};

use crate::wire_dto;

wire_dto! {
    /// One provider entry.
    pub struct Entry<'a>("LlmHealthEntry") dict entry(available: bool, detail: &'a str) {
        available: bool = available,
        detail: &'a str = detail,
    }
}

/// The health report: one entry per provider, in the order given.
pub fn health<'a>(providers: impl IntoIterator<Item = (&'a str, bool, &'a str)>) -> Value {
    Value::Dict(
        providers
            .into_iter()
            .map(|(name, available, detail)| (PyStr::from(name), entry(available, detail)))
            .collect(),
    )
}
