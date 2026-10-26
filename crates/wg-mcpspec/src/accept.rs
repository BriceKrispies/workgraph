//! The `Accept` negotiation of the Streamable HTTP endpoint (contract X6).
//!
//! The Python SDK (`mcp.server.streamable_http`, `check_accept_headers` and the checks in
//! `_validate_accept_header` and `handle_get`) decides from the `Accept` header alone whether a
//! POST or a GET may proceed in its default, SSE-answering mode. `*/*` satisfies both media types,
//! `application/*` the JSON one and `text/*` the event-stream one; anything else must name the
//! type outright. A refusal is a `406` with a JSON-RPC error body.

use serde_json::{Value, json};

use crate::jsonrpc::not_jsonrpc_reply;

/// The refusal message of a POST whose `Accept` lacks JSON or event-stream.
pub const POST_REFUSAL: &str =
    "Not Acceptable: Client must accept both application/json and text/event-stream";

/// The refusal message of a GET whose `Accept` lacks event-stream.
pub const GET_REFUSAL: &str = "Not Acceptable: Client must accept text/event-stream";

/// The `Accept` value the SDK behind this crate needs to take a request the Python rule has
/// passed. Its own check looks for both media types as substrings, which `*/*` does not satisfy.
pub const SDK_ACCEPT: &str = "application/json, text/event-stream";

/// The HTTP method of the request whose `Accept` is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Post,
    Get,
}

/// The media types an `Accept` header admits, as `(json, event_stream)`. Each comma-separated
/// entry is cut at its first `;`, trimmed and lowercased, as the Python SDK reads it.
#[must_use]
pub fn admitted_media(accept: &str) -> (bool, bool) {
    let types: Vec<String> = accept
        .split(',')
        .map(|entry| {
            entry
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .collect();
    let wildcard = types.iter().any(|t| t == "*/*");
    let json = wildcard
        || types
            .iter()
            .any(|t| t == "application/json" || t == "application/*");
    let sse = wildcard
        || types
            .iter()
            .any(|t| t == "text/event-stream" || t == "text/*");
    (json, sse)
}

/// The refusal message for a request with this method and `Accept` header (the empty string when
/// the header is absent), or `None` when the request may proceed.
#[must_use]
pub fn refusal(verb: Verb, accept: &str) -> Option<&'static str> {
    let (json, sse) = admitted_media(accept);
    match verb {
        Verb::Post => (!(json && sse)).then_some(POST_REFUSAL),
        Verb::Get => (!sse).then_some(GET_REFUSAL),
    }
}

/// The `406` body: a JSON-RPC error with no id and code `-32600` (INVALID_REQUEST), the shape
/// the Python SDK's `_create_error_response` writes.
#[must_use]
pub fn not_acceptable_reply(message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {"code": -32600, "message": message},
    })
}

/// The status of a refusal the endpoint answers itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// `406 Not Acceptable`: the `Accept` header is refused.
    NotAcceptable,
    /// `400 Bad Request`: a POST body that is not a JSON-RPC message (X6).
    BadRequest,
}

/// What the endpoint does with a request before the SDK sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    /// Answer now with this status and this JSON-RPC error body.
    Reply { reject: Reject, body: Value },
    /// Hand the request to the SDK.
    Forward,
}

/// The endpoint's own checks in the Python transport's order: the `Accept` negotiation first
/// (`406`), then, for a POST, the body that is not a JSON-RPC message (`400`, X6). `body` is the
/// whole request body, read by the caller; a GET ignores it.
#[must_use]
pub fn gate(verb: Verb, accept: &str, body: &[u8]) -> Gate {
    if let Some(message) = refusal(verb, accept) {
        return Gate::Reply {
            reject: Reject::NotAcceptable,
            body: not_acceptable_reply(message),
        };
    }
    match verb {
        Verb::Get => Gate::Forward,
        Verb::Post => not_jsonrpc_reply(body).map_or(Gate::Forward, |reply| Gate::Reply {
            reject: Reject::BadRequest,
            body: reply,
        }),
    }
}
