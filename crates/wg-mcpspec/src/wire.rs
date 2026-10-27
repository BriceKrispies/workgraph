//! Wire values as MCP structured content.
//!
//! The DTOs of `wg-wire` build `wg-json` values. A tool's structured output is the same JSON the
//! Python server dumps for its `dict`, so the value goes through the HTTP dialect's formatter and
//! is read back as a `serde_json` value that keeps the field order.

use serde_json::Value;
use wg_json::http_json;
use wg_types::WgError;

/// The structured content of a tool result for `value`.
pub fn structured(value: &wg_json::Value) -> Result<Value, WgError> {
    let text = http_json(value)
        .map_err(|err| WgError::workgraph(format!("cannot dump tool output: {err}")))?;
    serde_json::from_str(&text)
        .map_err(|err| WgError::workgraph(format!("tool output is not JSON: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_json::Value as Wire;

    #[test]
    fn keeps_field_order_and_nulls() {
        let wire = Wire::Dict(vec![("b".into(), Wire::Int(1)), ("a".into(), Wire::Null)]);
        let out = structured(&wire).map(|value| value.to_string());
        assert_eq!(out.ok().as_deref(), Some(r#"{"b":1,"a":null}"#));
    }
}
