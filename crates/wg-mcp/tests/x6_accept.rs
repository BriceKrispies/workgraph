//! X6: the `Accept` decision of the `/mcp/` endpoint matches the Python SDK's
//! `check_accept_headers` and its POST and GET checks (`mcp/server/streamable_http.py`).

#![allow(clippy::expect_used)]

use serde_json::json;
use wg_mcp::spec::accept::{GET_REFUSAL, POST_REFUSAL, admitted_media};
use wg_mcp::spec::{Verb, not_acceptable_reply, refusal};

// covers: X6
#[test]
fn a_wildcard_satisfies_both_media_types() {
    assert_eq!(admitted_media("*/*"), (true, true));
    assert_eq!(refusal(Verb::Post, "*/*"), None);
    assert_eq!(refusal(Verb::Get, "*/*"), None);
}

// covers: X6
#[test]
fn a_post_needs_json_and_event_stream() {
    assert_eq!(
        refusal(Verb::Post, "application/json, text/event-stream"),
        None
    );
    assert_eq!(refusal(Verb::Post, "application/json"), Some(POST_REFUSAL));
    assert_eq!(refusal(Verb::Post, "text/event-stream"), Some(POST_REFUSAL));
    assert_eq!(refusal(Verb::Post, ""), Some(POST_REFUSAL));
    assert_eq!(refusal(Verb::Post, "text/html"), Some(POST_REFUSAL));
}

// covers: X6
#[test]
fn a_get_needs_event_stream_only() {
    assert_eq!(refusal(Verb::Get, "text/event-stream"), None);
    assert_eq!(refusal(Verb::Get, "application/json"), Some(GET_REFUSAL));
    assert_eq!(refusal(Verb::Get, ""), Some(GET_REFUSAL));
}

// covers: X6
#[test]
fn subtype_wildcards_count_as_their_media_type() {
    assert_eq!(
        refusal(Verb::Post, "application/*, text/event-stream"),
        None
    );
    assert_eq!(refusal(Verb::Post, "application/json, text/*"), None);
    assert_eq!(refusal(Verb::Get, "text/*"), None);
    assert_eq!(
        refusal(Verb::Post, "application/xml, text/*"),
        Some(POST_REFUSAL)
    );
}

// covers: X6
#[test]
fn parameters_and_case_are_ignored_as_the_sdk_ignores_them() {
    assert_eq!(refusal(Verb::Post, "text/html;q=0.1, */*;q=0.5"), None);
    assert_eq!(
        refusal(Verb::Post, "Application/JSON ; q=1, TEXT/EVENT-STREAM"),
        None
    );
    assert_eq!(refusal(Verb::Get, "  text/event-stream  "), None);
}

// covers: X6
#[test]
fn the_refusal_messages_are_the_sdk_texts() {
    assert_eq!(
        POST_REFUSAL,
        "Not Acceptable: Client must accept both application/json and text/event-stream"
    );
    assert_eq!(
        GET_REFUSAL,
        "Not Acceptable: Client must accept text/event-stream"
    );
}

// covers: X6
#[test]
fn the_refusal_body_is_the_sdk_json_rpc_error() {
    let body = not_acceptable_reply(POST_REFUSAL);
    assert_eq!(
        serde_json::to_string(&body).expect("encode"),
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Not Acceptable: Client must accept both application/json and text/event-stream"}}"#
    );
    assert_eq!(
        body,
        json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {"code": -32600, "message": POST_REFUSAL},
        })
    );
}
