//! The answer to an authorized POST whose body is not a JSON-RPC message (contract X6).
//!
//! The Python SDK parses the body as `JSONRPCMessage`, a union of four Pydantic models, and
//! answers `400` with a JSON-RPC error whose message is the union's `str(ValidationError)`. The
//! Rust transport answers the same status and body before the SDK sees the request, so the
//! status does not depend on the `Accept` header.

use serde_json::{Map, Value, json};
use wg_validate::{Loc, ValidationError, pydantic::error_text};

/// The union the Python SDK validates against, as Pydantic titles it.
const UNION: &str = "union[JSONRPCRequest,JSONRPCNotification,JSONRPCResponse,JSONRPCError]";

/// The union members in order, each with the fields it requires in Pydantic's field order.
const MEMBERS: [(&str, &[&str]); 4] = [
    ("JSONRPCRequest", &["jsonrpc", "id", "method"]),
    ("JSONRPCNotification", &["jsonrpc", "method"]),
    ("JSONRPCResponse", &["jsonrpc", "id", "result"]),
    ("JSONRPCError", &["jsonrpc", "id", "error"]),
];

const PARSE_ERROR: i64 = -32700;
const INVALID_PARAMS: i64 = -32602;

/// The `400` body for `body` when it is not a JSON-RPC message, or `None` when a member of the
/// union is complete (the SDK then decides the rest).
#[must_use]
pub fn not_jsonrpc_reply(body: &[u8]) -> Option<Value> {
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(err) => return Some(error_reply(PARSE_ERROR, &format!("Parse error: {err}"))),
    };
    let is_complete = match &value {
        Value::Object(fields) => MEMBERS
            .iter()
            .any(|(_, required)| complete(fields, required)),
        _ => false,
    };
    if is_complete {
        return None;
    }
    let errors: Vec<ValidationError> = MEMBERS
        .iter()
        .flat_map(|(name, required)| member_errors(name, required, &value))
        .collect();
    let text = error_text(UNION, &errors);
    Some(error_reply(
        INVALID_PARAMS,
        &format!("Validation error: {text}"),
    ))
}

/// Whether an object has every required field of a member and the `2.0` version.
fn complete(fields: &Map<String, Value>, required: &[&str]) -> bool {
    fields.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && required.iter().all(|field| fields.contains_key(*field))
}

/// The errors one member reports for `value`: each missing field of an object, or one
/// `model_attributes_type` error for a value that is not an object.
fn member_errors(name: &str, required: &[&str], value: &Value) -> Vec<ValidationError> {
    let Value::Object(fields) = value else {
        return vec![ValidationError {
            kind: "model_attributes_type".to_owned(),
            loc: vec![Loc::Key(name.to_owned())],
            msg: "Input should be a valid dictionary or object to extract fields from".to_owned(),
            input: value.clone(),
            ctx: None,
        }];
    };
    required
        .iter()
        .filter(|field| !fields.contains_key(**field))
        .map(|field| ValidationError {
            kind: "missing".to_owned(),
            loc: vec![Loc::Key(name.to_owned()), Loc::Key((*field).to_owned())],
            msg: "Field required".to_owned(),
            input: value.clone(),
            ctx: None,
        })
        .collect()
}

/// A JSON-RPC error reply with no id.
fn error_reply(code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {"code": code, "message": message},
    })
}
