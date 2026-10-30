//! The `MCP-Protocol-Version` gate of the Streamable HTTP endpoint (WG-3819).
//!
//! The Python SDK (`mcp.server.streamable_http_manager._handle_request`) routes a request whose
//! `MCP-Protocol-Version` header names no handshake revision to its per-request entry
//! (`_streamable_http_modern.handle_modern_request`). That entry keeps no sessions, so the header
//! is checked before any session is looked up. A method other than POST is a `405` with
//! `Allow: POST`. A POST is checked in order: the `Content-Type` and `Accept` gates, the body as a
//! JSON-RPC message, the envelope (`shared/inbound.py`, `classify_inbound_request`), and last the
//! version itself, refused with `-32022` (UNSUPPORTED_PROTOCOL_VERSION). A notification is answered
//! with that error and a `null` id; a request carries its own id.
//!
//! The gate applies to every request, `initialize` included: the Python endpoint refuses an
//! `initialize` with such a header too, on its envelope check. Served versions (the handshake
//! revisions, and the per-request revision this crate does not serve) pass through.
//!
//! Known residual: an `Mcp-Name` value in the SDK's `=?base64?...?=` form is compared verbatim.

use serde_json::{Map, Value, json};

use crate::accept::{Plan, Reply, Verb, content_type_starts_json, first_header, refusal};

/// The header that names the protocol revision of a request.
pub const HEADER: &str = "mcp-protocol-version";

/// The revisions reachable through the `initialize` handshake (`HANDSHAKE_PROTOCOL_VERSIONS`).
const HANDSHAKE_VERSIONS: [&str; 4] = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];

/// The per-request revisions (`MODERN_PROTOCOL_VERSIONS`).
const MODERN_VERSIONS: [&str; 1] = ["2026-07-28"];

/// The `_meta` keys an envelope carries, in the order the SDK checks them.
const PROTOCOL_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";

/// The routing headers the SDK refuses to see twice.
const METHOD_HEADER: &str = "mcp-method";
const NAME_HEADER: &str = "mcp-name";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const INVALID_PARAMS: i64 = -32602;
const HEADER_MISMATCH: i64 = -32020;
const UNSUPPORTED_VERSION: i64 = -32022;

const INVALID_BODY: &str = "Body must be a single JSON-RPC request or notification object";
const META_MESSAGE: &str = "params._meta must be an object carrying the required \
'io.modelcontextprotocol/protocolVersion' and 'io.modelcontextprotocol/clientCapabilities' \
envelope keys";

/// The decision for a request whose `MCP-Protocol-Version` header names a version the endpoint
/// does not serve. `None` when the header is absent or names a served revision: the request then
/// takes the handshake path (or the SDK's own per-request path, which this crate does not model).
#[must_use]
pub fn gate(verb: Verb, headers: &[(String, String)], body: &[u8]) -> Option<Plan> {
    let version = first_header(headers, HEADER)?;
    if HANDSHAKE_VERSIONS.contains(&version) || MODERN_VERSIONS.contains(&version) {
        return None;
    }
    if verb != Verb::Post {
        return Some(Plan::Reply(Reply {
            status: 405,
            headers: vec![("allow".to_owned(), "POST".to_owned())],
            body: Vec::new(),
        }));
    }
    Some(post(headers, version, body))
}

/// The POST answer, in the per-request entry's order.
fn post(headers: &[(String, String)], version: &str, body: &[u8]) -> Plan {
    if !content_type_starts_json(headers) {
        return plain(400, "Invalid Content-Type header");
    }
    let accept = first_header(headers, "accept").unwrap_or_default();
    if refusal(Verb::Post, accept).is_some() {
        return Plan::Reply(Reply {
            status: 406,
            headers: Vec::new(),
            body: Vec::new(),
        });
    }
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return json_reply(&without_id(error(PARSE_ERROR, "Parse error", None))),
    };
    if let Some(fields) = value
        .as_object()
        .filter(|fields| !fields.contains_key("id"))
    {
        return if is_notification(fields) {
            json_reply(&without_id(unsupported(version)))
        } else {
            json_reply(&without_id(error(INVALID_REQUEST, INVALID_BODY, None)))
        };
    }
    if !is_request(&value) {
        return json_reply(&without_id(error(INVALID_REQUEST, INVALID_BODY, None)));
    }
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let failure = envelope_error(&value, headers, version).unwrap_or_else(|| unsupported(version));
    json_reply(&with_id(id, failure))
}

/// The first envelope rung that fails, as a JSON-RPC error object, or `None` when the envelope
/// passes and only the version is left to refuse.
fn envelope_error(value: &Value, headers: &[(String, String)], version: &str) -> Option<Value> {
    if let Some(name) = repeated_routing_header(headers) {
        return Some(error(
            HEADER_MISMATCH,
            &format!("{name} header appears more than once"),
            None,
        ));
    }
    let meta = value
        .get("params")
        .and_then(|params| params.get("_meta"))
        .and_then(Value::as_object);
    let Some(meta) = meta else {
        return Some(error(INVALID_PARAMS, META_MESSAGE, None));
    };
    let missing: Vec<&str> = [PROTOCOL_KEY, CAPABILITIES_KEY]
        .into_iter()
        .filter(|key| !meta.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Some(error(
            INVALID_PARAMS,
            &format!(
                "params._meta is missing the required envelope key(s): {}",
                missing.join(", ")
            ),
            None,
        ));
    }
    if meta.get(PROTOCOL_KEY) != Some(&Value::String(version.to_owned())) {
        return Some(error(
            HEADER_MISMATCH,
            "mcp-protocol-version header does not match the request envelope's protocol version",
            None,
        ));
    }
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if first_header(headers, METHOD_HEADER) != Some(method) {
        return Some(error(
            HEADER_MISMATCH,
            "mcp-method header does not match the request body's method",
            None,
        ));
    }
    if let Some(key) = name_key(method) {
        let body_value = value
            .get("params")
            .and_then(|params| params.get(key))
            .filter(|value| !value.is_null());
        if let Some(body_value) = body_value {
            match (first_header(headers, NAME_HEADER), body_value.as_str()) {
                (Some(header), Some(expected)) if header == expected => {}
                _ => {
                    return Some(error(
                        HEADER_MISMATCH,
                        &format!(
                            "mcp-name header does not match the request body's '{key}' parameter"
                        ),
                        None,
                    ));
                }
            }
        }
    }
    None
}

/// The params key whose value the `Mcp-Name` header mirrors, for the methods that carry one.
fn name_key(method: &str) -> Option<&'static str> {
    match method {
        "tools/call" | "prompts/get" => Some("name"),
        "resources/read" => Some("uri"),
        _ => None,
    }
}

/// The first routing header that occurs twice, by its lowercased name, in request order.
fn repeated_routing_header(headers: &[(String, String)]) -> Option<String> {
    let routing = [HEADER, METHOD_HEADER, NAME_HEADER];
    let mut seen: Vec<String> = Vec::new();
    for (name, _) in headers {
        let lower = name.to_ascii_lowercase();
        if !routing.contains(&lower.as_str()) {
            continue;
        }
        if seen.contains(&lower) {
            return Some(lower);
        }
        seen.push(lower);
    }
    None
}

/// The `-32022` error for a version the endpoint does not serve.
fn unsupported(version: &str) -> Value {
    error(
        UNSUPPORTED_VERSION,
        "Unsupported protocol version",
        Some(json!({"supported": MODERN_VERSIONS, "requested": version})),
    )
}

/// A JSON-RPC error object: code, message and, when present, data.
fn error(code: i64, message: &str, data: Option<Value>) -> Value {
    let mut fields = Map::new();
    fields.insert("code".to_owned(), json!(code));
    fields.insert("message".to_owned(), json!(message));
    if let Some(data) = data {
        fields.insert("data".to_owned(), data);
    }
    Value::Object(fields)
}

/// A reply to a request: `jsonrpc`, `id`, `error`, in the order the SDK writes them.
fn with_id(id: Value, error: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

/// A reply with no request id: `jsonrpc`, `error`, then `id: null`, as the SDK's `_write` writes it.
fn without_id(error: Value) -> Value {
    json!({"jsonrpc": "2.0", "error": error, "id": null})
}

/// A JSON reply with status 400, the status of every envelope rejection.
fn json_reply(body: &Value) -> Plan {
    Plan::Reply(Reply {
        status: 400,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        body: body.to_string().into_bytes(),
    })
}

/// A plain-text reply with no `Content-Type`.
fn plain(status: u16, message: &str) -> Plan {
    Plan::Reply(Reply {
        status,
        headers: Vec::new(),
        body: message.as_bytes().to_vec(),
    })
}

/// Whether an object is a JSON-RPC notification: a `2.0` message with a string method.
fn is_notification(fields: &Map<String, Value>) -> bool {
    fields.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && fields.get("method").is_some_and(Value::is_string)
}

/// Whether a value is a JSON-RPC request: a notification with an id that is a string or an integer.
fn is_request(value: &Value) -> bool {
    let Value::Object(fields) = value else {
        return false;
    };
    let id_ok = match fields.get("id") {
        Some(Value::String(_)) => true,
        Some(Value::Number(number)) => number.is_i64() || number.is_u64(),
        _ => false,
    };
    is_notification(fields) && id_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "1999-01-01";

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// The headers of a POST that passes the Content-Type and Accept gates, with `version`.
    fn post_headers(version: &str) -> Vec<(String, String)> {
        h(&[
            ("content-type", "application/json"),
            ("accept", "application/json, text/event-stream"),
            (HEADER, version),
        ])
    }

    fn meta(version: &str) -> Value {
        json!({PROTOCOL_KEY: version, CAPABILITIES_KEY: {}})
    }

    /// The reply of `plan`, or a panic naming it.
    fn reply(plan: Option<Plan>) -> Reply {
        match plan {
            Some(Plan::Reply(reply)) => reply,
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    fn body_text(reply: &Reply) -> &str {
        std::str::from_utf8(&reply.body).expect("utf-8 body")
    }

    #[test]
    fn an_absent_or_served_version_is_not_gated() {
        assert_eq!(gate(Verb::Post, &h(&[]), b"{}"), None);
        for version in ["2024-11-05", "2025-06-18", "2025-11-25", "2026-07-28"] {
            assert_eq!(
                gate(Verb::Post, &h(&[(HEADER, version)]), b"{}"),
                None,
                "{version}"
            );
        }
    }

    #[test]
    fn a_get_delete_or_other_method_is_a_405_allowing_post_with_no_body() {
        for verb in [Verb::Get, Verb::Delete, Verb::Other] {
            let reply = reply(gate(verb, &h(&[(HEADER, V)]), b""));
            assert_eq!(reply.status, 405);
            assert_eq!(reply.headers, vec![("allow".to_owned(), "POST".to_owned())]);
            assert!(reply.body.is_empty());
        }
    }

    #[test]
    fn a_non_json_content_type_is_a_plain_400_before_anything_else() {
        let headers = h(&[
            ("content-type", "text/plain"),
            ("accept", "*/*"),
            (HEADER, V),
        ]);
        let reply = reply(gate(Verb::Post, &headers, b"nope"));
        assert_eq!(reply.status, 400);
        assert!(reply.headers.is_empty());
        assert_eq!(body_text(&reply), "Invalid Content-Type header");
    }

    #[test]
    fn a_refused_accept_is_an_empty_406() {
        let headers = h(&[
            ("content-type", "application/json"),
            ("accept", "text/plain"),
            (HEADER, V),
        ]);
        let reply = reply(gate(Verb::Post, &headers, b"nope"));
        assert_eq!(reply.status, 406);
        assert!(reply.headers.is_empty());
        assert!(reply.body.is_empty());
    }

    #[test]
    fn an_unparsable_body_is_a_400_parse_error_with_null_id() {
        let reply = reply(gate(Verb::Post, &post_headers(V), b"nope"));
        assert_eq!(reply.status, 400);
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":null}"#
        );
    }

    #[test]
    fn a_batch_or_a_malformed_envelope_is_the_invalid_body_error() {
        let expected = r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"Body must be a single JSON-RPC request or notification object"},"id":null}"#;
        let batch = br#"[{"jsonrpc":"2.0","id":1,"method":"tools/list"}]"#;
        assert_eq!(
            body_text(&reply(gate(Verb::Post, &post_headers(V), batch))),
            expected
        );
        let no_version = br#"{"method":"x"}"#;
        assert_eq!(
            body_text(&reply(gate(Verb::Post, &post_headers(V), no_version))),
            expected
        );
    }

    #[test]
    fn a_notification_is_refused_with_the_version_error_and_a_null_id() {
        let body = br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let reply = reply(gate(Verb::Post, &post_headers(V), body));
        assert_eq!(reply.status, 400);
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","error":{"code":-32022,"message":"Unsupported protocol version","data":{"supported":["2026-07-28"],"requested":"1999-01-01"}},"id":null}"#
        );
    }

    #[test]
    fn a_request_without_params_meta_is_the_invalid_params_error_with_its_id() {
        let body = br#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#;
        let reply = reply(gate(Verb::Post, &post_headers(V), body));
        assert_eq!(reply.status, 400);
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","id":"a","error":{"code":-32602,"message":"params._meta must be an object carrying the required 'io.modelcontextprotocol/protocolVersion' and 'io.modelcontextprotocol/clientCapabilities' envelope keys"}}"#
        );
    }

    #[test]
    fn a_meta_missing_a_key_names_the_missing_keys() {
        let body = json!({
            "jsonrpc": "2.0", "id": "a", "method": "tools/list",
            "params": {"_meta": {PROTOCOL_KEY: V}},
        });
        let reply = reply(gate(
            Verb::Post,
            &post_headers(V),
            body.to_string().as_bytes(),
        ));
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","id":"a","error":{"code":-32602,"message":"params._meta is missing the required envelope key(s): io.modelcontextprotocol/clientCapabilities"}}"#
        );
    }

    #[test]
    fn a_meta_version_that_differs_from_the_header_is_a_header_mismatch() {
        let body = json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/list",
            "params": {"_meta": meta("2026-07-28")},
        });
        let headers = [post_headers(V), h(&[(METHOD_HEADER, "tools/list")])].concat();
        let reply = reply(gate(Verb::Post, &headers, body.to_string().as_bytes()));
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32020,"message":"mcp-protocol-version header does not match the request envelope's protocol version"}}"#
        );
    }

    #[test]
    fn a_request_without_the_mcp_method_header_is_a_header_mismatch() {
        let body = json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/list",
            "params": {"_meta": meta(V)},
        });
        let reply = reply(gate(
            Verb::Post,
            &post_headers(V),
            body.to_string().as_bytes(),
        ));
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32020,"message":"mcp-method header does not match the request body's method"}}"#
        );
    }

    #[test]
    fn a_name_bearing_request_must_mirror_its_name_in_the_header() {
        let body = json!({
            "jsonrpc": "2.0", "id": 8, "method": "tools/call",
            "params": {"_meta": meta(V), "name": "x"},
        });
        let text = body.to_string();
        let base = [post_headers(V), h(&[(METHOD_HEADER, "tools/call")])].concat();
        let missing = reply(gate(Verb::Post, &base, text.as_bytes()));
        assert_eq!(
            body_text(&missing),
            r#"{"jsonrpc":"2.0","id":8,"error":{"code":-32020,"message":"mcp-name header does not match the request body's 'name' parameter"}}"#
        );
        let wrong = [base.clone(), h(&[(NAME_HEADER, "y")])].concat();
        assert_eq!(reply(gate(Verb::Post, &wrong, text.as_bytes())).status, 400);
        let right = [base, h(&[(NAME_HEADER, "x")])].concat();
        assert_eq!(
            body_text(&reply(gate(Verb::Post, &right, text.as_bytes()))),
            r#"{"jsonrpc":"2.0","id":8,"error":{"code":-32022,"message":"Unsupported protocol version","data":{"supported":["2026-07-28"],"requested":"1999-01-01"}}}"#
        );
    }

    #[test]
    fn a_fully_formed_request_gets_the_version_error_with_its_id() {
        let body = json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/list",
            "params": {"_meta": meta(V)},
        });
        let headers = [post_headers(V), h(&[(METHOD_HEADER, "tools/list")])].concat();
        let reply = reply(gate(Verb::Post, &headers, body.to_string().as_bytes()));
        assert_eq!(reply.status, 400);
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32022,"message":"Unsupported protocol version","data":{"supported":["2026-07-28"],"requested":"1999-01-01"}}}"#
        );
    }

    #[test]
    fn a_repeated_routing_header_is_a_header_mismatch_before_the_envelope() {
        let body = json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/list",
            "params": {"_meta": meta(V)},
        });
        let headers = [post_headers(V), h(&[(HEADER, V)])].concat();
        let reply = reply(gate(Verb::Post, &headers, body.to_string().as_bytes()));
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32020,"message":"mcp-protocol-version header appears more than once"}}"#
        );
    }

    #[test]
    fn an_initialize_with_an_unserved_version_takes_the_envelope_path_too() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"p","version":"1"}}}"#;
        let reply = reply(gate(Verb::Post, &post_headers(V), body));
        assert_eq!(
            body_text(&reply),
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"params._meta must be an object carrying the required 'io.modelcontextprotocol/protocolVersion' and 'io.modelcontextprotocol/clientCapabilities' envelope keys"}}"#
        );
    }
}
