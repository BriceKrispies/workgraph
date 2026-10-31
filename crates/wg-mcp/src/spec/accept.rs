//! The `Accept` negotiation of the Streamable HTTP endpoint (contract X6).
//!
//! The Python SDK (`mcp.server.streamable_http`, `check_accept_headers` and the checks in
//! `_validate_accept_header` and `handle_get`) decides from the `Accept` header alone whether a
//! POST or a GET may proceed in its default, SSE-answering mode. `*/*` satisfies both media types,
//! `application/*` the JSON one and `text/*` the event-stream one; anything else must name the
//! type outright. A refusal is a `406` with a JSON-RPC error body.

use serde_json::{Value, json};

use crate::spec::jsonrpc::not_jsonrpc_reply;

/// The refusal message of a POST whose `Accept` lacks JSON or event-stream.
pub const POST_REFUSAL: &str =
    "Not Acceptable: Client must accept both application/json and text/event-stream";

/// The refusal message of a GET whose `Accept` lacks event-stream.
pub const GET_REFUSAL: &str = "Not Acceptable: Client must accept text/event-stream";

/// The `Accept` value the SDK behind this crate needs to take a request the Python rule has
/// passed. Its own check looks for both media types as substrings, which `*/*` does not satisfy.
pub const SDK_ACCEPT: &str = "application/json, text/event-stream";

/// The HTTP method of a request to the endpoint. Only POST and GET negotiate `Accept`; DELETE ends
/// a session; any other method is refused after the session lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Post,
    Get,
    Delete,
    Other,
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
        Verb::Delete | Verb::Other => None,
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
        Verb::Get | Verb::Delete | Verb::Other => Gate::Forward,
        Verb::Post => not_jsonrpc_reply(body).map_or(Gate::Forward, |reply| Gate::Reply {
            reject: Reject::BadRequest,
            body: reply,
        }),
    }
}

/// The HTTP method of a request as the endpoint's checks see it.
#[must_use]
pub fn verb_of(method: &str) -> Verb {
    match method {
        "POST" => Verb::Post,
        "GET" => Verb::Get,
        "DELETE" => Verb::Delete,
        _ => Verb::Other,
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
pub(crate) fn first_header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// A reply the endpoint writes itself: status, headers and body bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What the endpoint does with a request: answer it, or hand it to the SDK.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Answer now.
    Reply(Reply),
    /// Hand the request to the SDK with its `Accept` header replaced by this value.
    Forward { accept: &'static str },
}

/// The first decision, made from the headers alone: either the request is decided, or a POST
/// body must be read (under the endpoint's size limit) and given to [`check_body`].
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Answer(Plan),
    ReadBody,
}

/// The JSON-RPC error code of a request the endpoint refuses (INVALID_REQUEST).
const INVALID_REQUEST: i64 = -32600;

/// The message of a request whose session the SDK does not know.
const SESSION_NOT_FOUND: &str = "Session not found";

/// The message of a request without a session ID that is not an `initialize`.
const MISSING_SESSION: &str = "Bad Request: Missing session ID";

/// The message of a POST whose `Content-Type` is not exactly `application/json`.
const UNSUPPORTED_MEDIA: &str = "Unsupported Media Type: Content-Type must be application/json";

const JSON_TYPE: &str = "application/json";

/// The message of a method the endpoint does not serve.
const METHOD_NOT_ALLOWED: &str = "Method Not Allowed";

/// The methods the endpoint serves, as its `Allow` header lists them.
const ALLOW: &str = "GET, POST, DELETE";

/// A JSON-RPC error with no id, in the key order the Python SDK writes it.
#[must_use]
pub fn error_body(code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {"code": code, "message": message},
    })
}

/// A JSON reply that echoes `session` as its `Mcp-Session-Id`.
fn session_reply(status: u16, session: &str, body: &Value) -> Plan {
    Plan::Reply(Reply {
        status,
        headers: vec![
            ("content-type".to_owned(), JSON_TYPE.to_owned()),
            ("mcp-session-id".to_owned(), session.to_owned()),
        ],
        body: body.to_string().into_bytes(),
    })
}

/// A plain-text reply. Starlette writes a bare `Response("...")` with no `Content-Type`, and so
/// does this.
fn text_reply(status: u16, message: &str) -> Plan {
    Plan::Reply(Reply {
        status,
        headers: Vec::new(),
        body: message.as_bytes().to_vec(),
    })
}

/// The reply to a request whose body is over the limit: `413`, plain text with no `Content-Type`.
/// The Python body-limit middleware wraps the whole endpoint, so this is decided before any other
/// check, for every method, as soon as the body is known to be over the limit.
#[must_use]
pub fn too_large() -> Reply {
    Reply {
        status: 413,
        headers: Vec::new(),
        body: b"Request body too large".to_vec(),
    }
}

/// The Python transport's `Content-Type` gate for a POST: the lowercased header must start with
/// `application/json` (`TransportSecurityMiddleware._validate_content_type`).
pub(crate) fn content_type_starts_json(headers: &[(String, String)]) -> bool {
    first_header(headers, "content-type")
        .is_some_and(|ct| ct.to_ascii_lowercase().starts_with(JSON_TYPE))
}

/// The SDK's exact gate (`_check_content_type`): one of the comma-separated parts before the
/// first `;` must equal `application/json`.
fn content_type_is_json(headers: &[(String, String)]) -> bool {
    first_header(headers, "content-type").is_some_and(|ct| {
        ct.split(';')
            .next()
            .unwrap_or_default()
            .split(',')
            .any(|part| part.trim() == JSON_TYPE)
    })
}

/// Whether a body is a JSON-RPC `initialize` request (an object with a `method` of `initialize`
/// and an `id`), which may open a session without one.
fn is_initialize(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body).is_ok_and(|value| {
        value.get("method").and_then(Value::as_str) == Some("initialize")
            && value.get("id").is_some()
    })
}

/// The decision that needs only the headers, in the Python transport's order (after the body-size
/// `413` the caller makes first, see [`too_large`]): an unknown session
/// is `404` before anything else (`_handle_stateful_request`); a POST whose `Content-Type` does
/// not start with `application/json` is a plain `400` (the transport's security check); a refused
/// `Accept` is `406`; a GET or DELETE without a session ID is `400` (`_validate_session`); any
/// other method is `405` (`_handle_unsupported_request`). `session_known`
/// says whether the request's `Mcp-Session-Id`, when it has one, names a live session.
/// `fresh_session` is the id a reply echoes when the request carries none.
#[must_use]
pub fn check_headers(
    verb: Verb,
    headers: &[(String, String)],
    session_known: bool,
    fresh_session: &str,
) -> Step {
    let accept = first_header(headers, "accept").unwrap_or_default();
    let request_session = first_header(headers, "mcp-session-id");
    let session = request_session.unwrap_or(fresh_session);
    if request_session.is_some() && !session_known {
        return Step::Answer(Plan::Reply(Reply {
            status: 404,
            headers: vec![("content-type".to_owned(), JSON_TYPE.to_owned())],
            body: error_body(INVALID_REQUEST, SESSION_NOT_FOUND)
                .to_string()
                .into_bytes(),
        }));
    }
    if verb == Verb::Other {
        return Step::Answer(Plan::Reply(Reply {
            status: 405,
            headers: vec![
                ("content-type".to_owned(), JSON_TYPE.to_owned()),
                ("allow".to_owned(), ALLOW.to_owned()),
                ("mcp-session-id".to_owned(), session.to_owned()),
            ],
            body: error_body(INVALID_REQUEST, METHOD_NOT_ALLOWED)
                .to_string()
                .into_bytes(),
        }));
    }
    if verb == Verb::Post && !content_type_starts_json(headers) {
        return Step::Answer(text_reply(400, "Invalid Content-Type header"));
    }
    if let Some(message) = refusal(verb, accept) {
        return Step::Answer(session_reply(406, session, &not_acceptable_reply(message)));
    }
    match verb {
        Verb::Post => Step::ReadBody,
        Verb::Get | Verb::Delete if request_session.is_none() => Step::Answer(session_reply(
            400,
            session,
            &error_body(INVALID_REQUEST, MISSING_SESSION),
        )),
        Verb::Get | Verb::Delete | Verb::Other => {
            Step::Answer(Plan::Forward { accept: SDK_ACCEPT })
        }
    }
}

/// What the endpoint changes in a response the SDK has produced for a forwarded request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Touch {
    /// A status to write instead of the SDK's.
    pub status: Option<u16>,
    /// Headers to set on the response.
    pub headers: Vec<(String, String)>,
}

/// How the Python transport answers what the SDK answers differently: a DELETE that ended a
/// session is `200` and an accepted POST (a notification or a response) is `202`, each as a JSON
/// response echoing the session ID, with no body. `status` is the SDK's status and `session` the
/// request's session ID (empty when it has none).
#[must_use]
pub fn touch_up(verb: Verb, status: u16, session: &str) -> Touch {
    let ends_session = verb == Verb::Delete && status < 400;
    let accepted = verb == Verb::Post && status == 202;
    let mut headers = Vec::new();
    if ends_session || accepted {
        headers.push(("content-type".to_owned(), JSON_TYPE.to_owned()));
    }
    if !session.is_empty() {
        headers.push(("mcp-session-id".to_owned(), session.to_owned()));
    }
    Touch {
        status: ends_session.then_some(200),
        headers,
    }
}

/// The decision that needs the POST body, in the Python transport's order: the exact
/// `application/json` gate is `415`; a body that is not a JSON-RPC message is `400`; a request
/// without a session ID that is not an `initialize` is `400`. Anything else goes to the SDK.
#[must_use]
pub fn check_body(headers: &[(String, String)], fresh_session: &str, body: &[u8]) -> Plan {
    let request_session = first_header(headers, "mcp-session-id");
    let session = request_session.unwrap_or(fresh_session);
    if !content_type_is_json(headers) {
        return session_reply(
            415,
            session,
            &error_body(INVALID_REQUEST, UNSUPPORTED_MEDIA),
        );
    }
    if let Some(reply) = not_jsonrpc_reply(body) {
        return session_reply(400, session, &reply);
    }
    if request_session.is_none() && !is_initialize(body) {
        return session_reply(400, session, &error_body(INVALID_REQUEST, MISSING_SESSION));
    }
    Plan::Forward { accept: SDK_ACCEPT }
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

    fn reply_of(step: Step) -> Reply {
        match step {
            Step::Answer(Plan::Reply(reply)) => reply,
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    #[test]
    fn only_post_and_get_are_checked() {
        assert_eq!(verb_of("POST"), Verb::Post);
        assert_eq!(verb_of("GET"), Verb::Get);
        assert_eq!(verb_of("DELETE"), Verb::Delete);
        assert_eq!(verb_of("PUT"), Verb::Other);
    }

    #[test]
    fn statuses_are_the_http_numbers() {
        assert_eq!(reject_status(Reject::NotAcceptable), 406);
        assert_eq!(reject_status(Reject::BadRequest), 400);
    }

    #[test]
    fn an_accepted_post_reads_its_body_and_an_initialize_is_forwarded() {
        let headers = h(&[("accept", "*/*"), ("content-type", "application/json")]);
        assert_eq!(
            check_headers(Verb::Post, &headers, false, "fresh"),
            Step::ReadBody
        );
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        assert_eq!(
            check_body(&headers, "fresh", body),
            Plan::Forward { accept: SDK_ACCEPT }
        );
    }

    #[test]
    fn plain_replies_carry_no_content_type() {
        let headers = h(&[("accept", "text/plain"), ("content-type", "text/plain")]);
        let reply = reply_of(check_headers(Verb::Post, &headers, false, "fresh"));
        assert!(reply.headers.is_empty());
        let big = too_large();
        assert_eq!((big.status, big.headers.is_empty()), (413, true));
        assert_eq!(big.body, b"Request body too large".to_vec());
    }

    #[test]
    fn delete_follows_the_session_rules_of_a_get_without_negotiating_accept() {
        let none = h(&[]);
        let reply = reply_of(check_headers(Verb::Delete, &none, false, "fresh"));
        assert_eq!(reply.status, 400);
        assert!(String::from_utf8_lossy(&reply.body).contains("Missing session ID"));
        let gone = h(&[("mcp-session-id", "gone")]);
        assert_eq!(
            reply_of(check_headers(Verb::Delete, &gone, false, "fresh")).status,
            404
        );
        let live = h(&[("mcp-session-id", "live"), ("accept", "text/plain")]);
        assert_eq!(
            check_headers(Verb::Delete, &live, true, "fresh"),
            Step::Answer(Plan::Forward { accept: SDK_ACCEPT })
        );
    }

    #[test]
    fn another_method_is_a_405_after_the_session_lookup() {
        let none = h(&[]);
        let reply = reply_of(check_headers(Verb::Other, &none, false, "fresh"));
        assert_eq!(reply.status, 405);
        assert!(
            reply
                .headers
                .contains(&("allow".to_owned(), "GET, POST, DELETE".to_owned()))
        );
        assert!(
            reply
                .headers
                .contains(&("mcp-session-id".to_owned(), "fresh".to_owned()))
        );
        assert_eq!(
            String::from_utf8_lossy(&reply.body),
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Method Not Allowed"}}"#
        );
        let gone = h(&[("mcp-session-id", "gone")]);
        assert_eq!(
            reply_of(check_headers(Verb::Other, &gone, false, "fresh")).status,
            404
        );
    }

    #[test]
    fn a_delete_the_sdk_accepted_is_a_200_and_an_accepted_post_a_json_202() {
        let ended = touch_up(Verb::Delete, 202, "s");
        assert_eq!(ended.status, Some(200));
        assert!(
            ended
                .headers
                .contains(&("mcp-session-id".to_owned(), "s".to_owned()))
        );
        let accepted = touch_up(Verb::Post, 202, "s");
        assert_eq!(accepted.status, None);
        assert!(
            accepted
                .headers
                .contains(&("content-type".to_owned(), JSON_TYPE.to_owned()))
        );
        assert_eq!(touch_up(Verb::Post, 200, ""), Touch::default());
        assert_eq!(touch_up(Verb::Delete, 404, ""), Touch::default());
        let echoed = touch_up(Verb::Get, 200, "s");
        assert_eq!(echoed.status, None);
        assert_eq!(
            echoed.headers,
            vec![("mcp-session-id".to_owned(), "s".to_owned())]
        );
    }

    #[test]
    fn an_unknown_session_is_a_404_before_the_accept_check() {
        let headers = h(&[("accept", "text/plain"), ("Mcp-Session-Id", "gone")]);
        let reply = reply_of(check_headers(Verb::Post, &headers, false, "fresh"));
        assert_eq!(reply.status, 404);
        assert!(reply.headers.iter().all(|(k, _)| k != "mcp-session-id"));
        assert_eq!(
            String::from_utf8_lossy(&reply.body),
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Session not found"}}"#
        );
    }

    #[test]
    fn a_refused_accept_is_a_406_echoing_the_request_session() {
        let headers = h(&[
            ("accept", "text/plain"),
            ("content-type", "application/json"),
            ("Mcp-Session-Id", "abc"),
        ]);
        let reply = reply_of(check_headers(Verb::Post, &headers, true, "fresh"));
        assert_eq!(reply.status, 406);
        assert!(
            reply
                .headers
                .contains(&("mcp-session-id".to_owned(), "abc".to_owned()))
        );
        assert!(
            reply
                .headers
                .contains(&("content-type".to_owned(), JSON_TYPE.to_owned()))
        );
        assert!(String::from_utf8_lossy(&reply.body).contains("-32600"));
    }

    #[test]
    fn a_non_json_content_type_is_a_plain_400_before_the_accept_check() {
        let headers = h(&[("accept", "text/plain"), ("content-type", "text/plain")]);
        let reply = reply_of(check_headers(Verb::Post, &headers, false, "fresh"));
        assert_eq!(reply.status, 400);
        assert_eq!(reply.body, b"Invalid Content-Type header".to_vec());
    }

    #[test]
    fn a_json_prefixed_content_type_that_is_not_exact_is_a_415() {
        let headers = h(&[("accept", "*/*"), ("content-type", "application/json-seq")]);
        assert_eq!(
            check_headers(Verb::Post, &headers, false, "fresh"),
            Step::ReadBody
        );
        match check_body(&headers, "fresh", b"{}") {
            Plan::Reply(reply) => assert_eq!(reply.status, 415),
            Plan::Forward { .. } => panic!("forwarded"),
        }
    }

    #[test]
    fn a_post_without_session_that_is_not_an_initialize_is_a_400_missing_session() {
        let headers = h(&[("accept", "*/*"), ("content-type", "application/json")]);
        let body = br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        match check_body(&headers, "fresh", body) {
            Plan::Reply(reply) => {
                assert_eq!(reply.status, 400);
                assert!(
                    reply
                        .headers
                        .contains(&("mcp-session-id".to_owned(), "fresh".to_owned()))
                );
                assert!(String::from_utf8_lossy(&reply.body).contains("Missing session ID"));
            }
            Plan::Forward { .. } => panic!("forwarded"),
        }
    }

    #[test]
    fn a_get_without_session_is_a_400_after_the_accept_check() {
        let ok = h(&[("accept", "text/event-stream")]);
        let reply = reply_of(check_headers(Verb::Get, &ok, false, "fresh"));
        assert_eq!(reply.status, 400);
        assert!(
            reply
                .headers
                .contains(&("mcp-session-id".to_owned(), "fresh".to_owned()))
        );
        let refused = h(&[("accept", "text/plain")]);
        assert_eq!(
            reply_of(check_headers(Verb::Get, &refused, false, "fresh")).status,
            406
        );
    }

    #[test]
    fn a_non_jsonrpc_post_body_is_a_400() {
        let headers = h(&[
            ("accept", "*/*"),
            ("content-type", "application/json"),
            ("mcp-session-id", "s"),
        ]);
        match check_body(&headers, "fresh", b"{}") {
            Plan::Reply(reply) => assert_eq!(reply.status, 400),
            Plan::Forward { .. } => panic!("forwarded"),
        }
    }
}
