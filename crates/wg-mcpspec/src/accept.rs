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

/// The HTTP method of a request as the endpoint's checks see it: only POST and GET are checked.
#[must_use]
pub fn verb_of(method: &str) -> Option<Verb> {
    match method {
        "POST" => Some(Verb::Post),
        "GET" => Some(Verb::Get),
        _ => None,
    }
}

/// The HTTP status number of a refusal.
#[must_use]
pub const fn reject_status(reject: Reject) -> u16 {
    match reject {
        Reject::NotAcceptable => 406,
        Reject::BadRequest => 400,
    }
}

/// The request header named `name` (case-insensitive), first occurrence.
fn first_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// What the endpoint does with a POST or GET, described in plain values.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Answer now with this status, these headers and this body.
    Reply {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Hand the request to the SDK with its `Accept` header replaced by this value.
    Forward { accept: &'static str },
}

/// The whole decision for a request: `headers` are the request's, `fresh_session` is the id to
/// echo when the request carries none, and `body` is the whole request body. A reply carries the
/// JSON content type and the session id the Python transport echoes (the request's own, or the
/// fresh one).
#[must_use]
pub fn plan(verb: Verb, headers: &[(String, String)], fresh_session: &str, body: &[u8]) -> Plan {
    let accept = first_header(headers, "accept").unwrap_or_default();
    let session = first_header(headers, "mcp-session-id").unwrap_or(fresh_session);
    match gate(verb, accept, body) {
        Gate::Forward => Plan::Forward { accept: SDK_ACCEPT },
        Gate::Reply { reject, body } => Plan::Reply {
            status: reject_status(reject),
            headers: vec![
                ("content-type".to_owned(), "application/json".to_owned()),
                ("mcp-session-id".to_owned(), session.to_owned()),
            ],
            body: body.to_string().into_bytes(),
        },
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn only_post_and_get_are_checked() {
        assert_eq!(verb_of("POST"), Some(Verb::Post));
        assert_eq!(verb_of("GET"), Some(Verb::Get));
        assert_eq!(verb_of("DELETE"), None);
    }

    #[test]
    fn statuses_are_the_http_numbers() {
        assert_eq!(reject_status(Reject::NotAcceptable), 406);
        assert_eq!(reject_status(Reject::BadRequest), 400);
    }

    #[test]
    fn an_accepted_request_is_forwarded_with_the_widened_accept() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let got = plan(Verb::Post, &h(&[("Accept", "*/*")]), "fresh", body);
        assert_eq!(got, Plan::Forward { accept: SDK_ACCEPT });
    }

    #[test]
    fn a_refused_accept_is_a_406_echoing_the_request_session() {
        let headers = h(&[("accept", "text/plain"), ("Mcp-Session-Id", "abc")]);
        match plan(Verb::Post, &headers, "fresh", b"{}") {
            Plan::Reply {
                status,
                headers,
                body,
            } => {
                assert_eq!(status, 406);
                assert!(headers.contains(&("mcp-session-id".to_owned(), "abc".to_owned())));
                assert!(
                    headers.contains(&("content-type".to_owned(), "application/json".to_owned()))
                );
                assert!(String::from_utf8_lossy(&body).contains("-32600"));
            }
            Plan::Forward { .. } => panic!("forwarded"),
        }
    }

    #[test]
    fn a_missing_accept_and_session_use_empty_accept_and_the_fresh_id() {
        match plan(Verb::Get, &[], "fresh", b"") {
            Plan::Reply {
                status, headers, ..
            } => {
                assert_eq!(status, 406);
                assert!(headers.contains(&("mcp-session-id".to_owned(), "fresh".to_owned())));
            }
            Plan::Forward { .. } => panic!("forwarded"),
        }
    }

    #[test]
    fn a_non_jsonrpc_post_body_is_a_400() {
        match plan(Verb::Post, &h(&[("accept", "*/*")]), "fresh", b"{}") {
            Plan::Reply { status, .. } => assert_eq!(status, 400),
            Plan::Forward { .. } => panic!("forwarded"),
        }
    }
}
