//! X6: the Streamable HTTP decisions of the `/mcp/` endpoint, in the Python transport's order,
//! and the SSE framing of the Python SDK (`mcp/server/streamable_http.py`).

#![allow(clippy::expect_used)]

use wg_mcpspec::{Plan, Step, Verb, check_body, check_headers, frame_events};

fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn status_of(step: Step) -> u16 {
    match step {
        Step::Answer(Plan::Reply(reply)) => reply.status,
        other => panic!("expected a reply, got {other:?}"),
    }
}

// covers: X6
#[test]
fn an_unknown_session_is_404_even_when_the_accept_is_refused() {
    let h = headers(&[("accept", "text/plain"), ("mcp-session-id", "gone")]);
    assert_eq!(
        status_of(check_headers(Verb::Post, &h, false, "fresh")),
        404
    );
}

// covers: X6
#[test]
fn a_known_session_with_a_refused_accept_is_406() {
    let h = headers(&[
        ("accept", "text/plain"),
        ("content-type", "application/json"),
        ("mcp-session-id", "live"),
    ]);
    assert_eq!(status_of(check_headers(Verb::Post, &h, true, "fresh")), 406);
}

// covers: X6
#[test]
fn a_post_is_read_only_after_the_headers_pass() {
    let h = headers(&[("accept", "*/*"), ("content-type", "application/json")]);
    assert_eq!(
        check_headers(Verb::Post, &h, false, "fresh"),
        Step::ReadBody
    );
}

// covers: X6
#[test]
fn a_content_type_that_is_json_prefixed_but_not_exact_is_415_after_the_body_is_read() {
    let h = headers(&[("accept", "*/*"), ("content-type", "application/json-seq")]);
    assert_eq!(
        check_headers(Verb::Post, &h, false, "fresh"),
        Step::ReadBody
    );
    match check_body(&h, "fresh", b"{}") {
        Plan::Reply(reply) => assert_eq!(reply.status, 415),
        Plan::Forward { .. } => panic!("forwarded"),
    }
}

// covers: X6
#[test]
fn an_initialize_without_a_session_is_forwarded() {
    let h = headers(&[("accept", "*/*"), ("content-type", "application/json")]);
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    assert!(matches!(
        check_body(&h, "fresh", body),
        Plan::Forward { .. }
    ));
}

// covers: X6
#[test]
fn a_get_without_a_session_is_400_missing_session() {
    let h = headers(&[("accept", "text/event-stream")]);
    let reply = match check_headers(Verb::Get, &h, false, "fresh") {
        Step::Answer(Plan::Reply(reply)) => reply,
        other => panic!("expected a reply, got {other:?}"),
    };
    assert_eq!(reply.status, 400);
    assert_eq!(
        String::from_utf8_lossy(&reply.body),
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Bad Request: Missing session ID"}}"#
    );
}

// covers: X6
#[test]
fn a_message_event_is_framed_as_the_python_sdk_writes_it() {
    let input = b"id: 0/1\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
    let (out, used) = frame_events(input);
    assert_eq!(used, input.len());
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\r\n\r\n"
    );
}

// covers: X6
#[test]
fn a_partial_event_waits_until_its_blank_line_arrives() {
    let (first, used) = frame_events(b"data: {\"a\":");
    assert!(first.is_empty());
    assert_eq!(used, 0);
    let (second, _) = frame_events(b"data: {\"a\":1}\n\n");
    assert_eq!(
        String::from_utf8(second).expect("utf-8"),
        "event: message\r\ndata: {\"a\":1}\r\n\r\n"
    );
}
