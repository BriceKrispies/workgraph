//! The readers of the registry routes' validated request (IF-42, IF-43): the query parameters and
//! the body fields a handler takes, and the error of an operation id the table does not name. Pure
//! functions over the parameter map and the body, so they live here and the route table in
//! `http` keeps only the glue.

use serde_json::{Map, Value as Json};
use wg_types::WgError;

/// A boolean query parameter, `false` when unset.
#[must_use]
pub fn flag(params: &Map<String, Json>, name: &str) -> bool {
    params.get(name).and_then(Json::as_bool).unwrap_or(false)
}

/// An integer query parameter, `default` when unset.
#[must_use]
pub fn number(params: &Map<String, Json>, name: &str, default: i64) -> i64 {
    params.get(name).and_then(Json::as_i64).unwrap_or(default)
}

/// A text field of the validated body, empty when absent.
#[must_use]
pub fn body_text<'a>(body: &'a Json, key: &str) -> &'a str {
    body.get(key).and_then(Json::as_str).unwrap_or_default()
}

/// The error of an operation id the route table does not name.
#[must_use]
pub fn no_handler(id: &str) -> WgError {
    WgError::workgraph(format!("route {id} has no handler"))
}
