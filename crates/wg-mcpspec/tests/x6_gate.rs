//! X6: the endpoint's checks before the SDK, in the Python transport's order: the `Accept`
//! negotiation (`406`), then a POST body that is not a JSON-RPC message (`400`).

#![allow(clippy::expect_used)]

use wg_mcpspec::{Gate, Reject, Verb, gate};

const INITIALIZE: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;

// covers: X6
#[test]
fn a_wildcard_initialize_is_forwarded() {
    assert_eq!(gate(Verb::Post, "*/*", INITIALIZE), Gate::Forward);
}

// covers: X6
#[test]
fn a_refused_accept_is_answered_before_the_body_is_looked_at() {
    match gate(Verb::Post, "application/json", b"{}") {
        Gate::Reply { reject, body } => {
            assert_eq!(reject, Reject::NotAcceptable);
            assert_eq!(body["error"]["code"], -32600);
        }
        Gate::Forward => panic!("refused Accept was forwarded"),
    }
}

// covers: X6
#[test]
fn a_post_body_that_is_not_jsonrpc_is_a_400_once_accept_passes() {
    match gate(Verb::Post, "*/*", b"{}") {
        Gate::Reply { reject, body } => {
            assert_eq!(reject, Reject::BadRequest);
            assert_eq!(body["error"]["code"], -32602);
        }
        Gate::Forward => panic!("non-JSON-RPC body was forwarded"),
    }
}

// covers: X6
#[test]
fn a_get_is_forwarded_once_its_accept_passes_whatever_the_body() {
    assert_eq!(gate(Verb::Get, "*/*", b"{}"), Gate::Forward);
    assert_eq!(gate(Verb::Get, "text/event-stream", b""), Gate::Forward);
}

// covers: X6
#[test]
fn a_get_with_a_refused_accept_is_a_406() {
    assert!(matches!(
        gate(Verb::Get, "application/json", b""),
        Gate::Reply {
            reject: Reject::NotAcceptable,
            ..
        }
    ));
}
