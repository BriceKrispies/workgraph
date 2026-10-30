//! `tools/call` results and the error mapping of `tool_errors` (`server.py`).
//!
//! A tool never fails at the protocol level once it is found and its arguments parsed: a domain
//! error becomes a result with `isError: true` whose text is the SDK's
//! `Error executing tool <name>: <message>`, where `<message>` is the error envelope as
//! `json.dumps({"error": exc.to_dict()}, default=str)`.

use serde_json::{Map, Value, json};
use wg_json::log_json;
use wg_types::WgError;
use wg_wire::error::error_envelope;

use crate::mcp::arguments::check_arguments;
use crate::mcp::catalog::Catalog;

/// The message of the JSON-RPC `-32602` error for a request whose params do not parse (its
/// `data` is the empty string).
pub const INVALID_PARAMS_MESSAGE: &str = "Invalid request parameters";

/// Whether `method` is one the server implements, so that a request for it which reached the
/// fallback handler failed to parse (`-32602`) rather than named an unknown method (`-32601`).
pub fn has_params_to_parse(method: &str) -> bool {
    matches!(
        method,
        "tools/call"
            | "resources/read"
            | "resources/subscribe"
            | "resources/unsubscribe"
            | "prompts/get"
            | "completion/complete"
            | "logging/setLevel"
    )
}

/// The text of a call to a tool that is not registered.
pub fn unknown_tool_text(name: &str) -> String {
    format!("Unknown tool: {name}")
}

/// The SDK's wrapper around whatever message a failing tool produced.
pub fn execution_error_text(name: &str, message: &str) -> String {
    format!("Error executing tool {name}: {message}")
}

/// The message `tool_errors` gives a `WorkgraphError`: the envelope in Python's default JSON.
pub fn tool_error_text(name: &str, error: &WgError) -> String {
    let body = log_json(&error_envelope(error))
        .unwrap_or_else(|err| format!("{{\"error\": {{\"message\": \"{err}\"}}}}"));
    execution_error_text(name, &body)
}

/// A failed call: one text item and `isError: true`.
pub fn error_result(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

/// The structured output of a tool, from a wire value (`wg-json` dialect) into the `serde_json`
/// value the result carries. Key order is kept, so the indented text matches the Python dump.
pub fn structured_output(value: &wg_json::Value) -> Result<Value, WgError> {
    let text = wg_json::http_json(value).map_err(|err| WgError::workgraph(err.to_string()))?;
    serde_json::from_str(&text).map_err(|err| WgError::workgraph(err.to_string()))
}

/// The string argument `key` of a call, when it is given and not empty (Python's
/// `workspace or default`).
pub fn text_argument<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// A successful call: the structured output, also as indented JSON text for clients that read
/// `content`.
pub fn success_result(structured: &Value) -> Value {
    let text = serde_json::to_string_pretty(structured).unwrap_or_default();
    let mut out = Map::new();
    out.insert("content".into(), json!([{"type": "text", "text": text}]));
    out.insert("structuredContent".into(), structured.clone());
    out.insert("isError".into(), Value::Bool(false));
    Value::Object(out)
}

/// The `resources/read` result for a JSON document `text` read from `uri`.
///
/// The recorded reply's item has `uri`, `mimeType` and `text`, which is what `rmcp`'s resource
/// contents type requires (`wg3738_resources` test).
pub fn read_result(uri: &str, text: &str) -> Value {
    json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": text}]})
}

/// Answers `tools/call` for `name`: an unknown tool and invalid arguments are `isError` results
/// without running anything; otherwise `run` executes the tool with the validated arguments and
/// its outcome is shaped as `tool_errors` does.
pub fn respond_to_call(
    catalog: &Catalog,
    name: &str,
    arguments: Option<&Value>,
    run: impl FnOnce(Value) -> Result<Value, WgError>,
) -> Value {
    let Some(spec) = catalog.tool(name) else {
        return error_result(&unknown_tool_text(name));
    };
    match check_arguments(spec, arguments) {
        Err(err) => error_result(&execution_error_text(name, &err.text())),
        Ok(valid) => match run(valid) {
            Ok(structured) => success_result(&structured),
            Err(err) => error_result(&tool_error_text(name, &err)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_error_text_matches_the_transcript() {
        let error = WgError::not_found("workspace 'x' not found").detail("ref", "x");
        assert_eq!(
            tool_error_text("workspace_get", &error),
            "Error executing tool workspace_get: {\"error\": {\"code\": \"not_found\", \"message\": \"workspace 'x' not found\", \"details\": {\"ref\": \"x\"}}}"
        );
    }
}
