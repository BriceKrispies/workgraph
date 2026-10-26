//! The error envelope (contracts O02, O06, O09).
//!
//! Every error the HTTP API and the MCP server return, and every `--json` error the CLI prints, is
//! `{"error": {"code": ..., "message": ..., "details": {...}}}`. The CLI prints it compact on one
//! line, HTTP sets the status from the error class, and MCP puts it in the text of an `isError`
//! result.

use serde_json::{Map, Value as Json};
use wg_json::{PyStr, Value};
use wg_types::{ErrorCode, WgError};

use crate::convention::{json_object, json_value, text};

/// The code of an error the CLI raises itself with `fail()` (O02).
pub const CLI_ERROR_CODE: &str = "cli_error";

/// The message of a request that failed validation (O06).
pub const INVALID_REQUEST_MESSAGE: &str = "invalid request";

/// The HTTP status of a request validation failure.
pub const VALIDATION_STATUS: u16 = 422;

fn key(name: &str) -> PyStr {
    PyStr::from(name)
}

/// The inner object `{"code", "message", "details"}` (Python `WorkgraphError.to_dict()`).
pub fn error_body(code: &str, message: &str, details: &Map<String, Json>) -> Value {
    Value::Dict(vec![
        (key("code"), text(code)),
        (key("message"), text(message)),
        (key("details"), json_object(details)),
    ])
}

fn envelope(body: Value) -> Value {
    Value::Dict(vec![(key("error"), body)])
}

/// The envelope for a domain error.
pub fn error_envelope(error: &WgError) -> Value {
    envelope(error_body(
        error.code.as_str(),
        &error.message,
        &error.details,
    ))
}

/// The envelope for an error the CLI raises itself (`fail()`), which has no details.
pub fn cli_error_envelope(message: &str) -> Value {
    envelope(error_body(CLI_ERROR_CODE, message, &Map::new()))
}

/// The 422 envelope for a request whose body, query or arguments failed validation. `errors` are
/// the Pydantic error items (`wg_validate::ValidationError::to_json`).
pub fn validation_envelope(errors: &[Json]) -> Value {
    let details = Value::Dict(vec![(
        key("errors"),
        Value::List(errors.iter().map(json_value).collect()),
    )]);
    envelope(Value::Dict(vec![
        (key("code"), text(ErrorCode::ValidationFailed.as_str())),
        (key("message"), text(INVALID_REQUEST_MESSAGE)),
        (key("details"), details),
    ]))
}
