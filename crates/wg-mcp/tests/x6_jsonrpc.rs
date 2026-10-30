//! X6: an authorized POST whose body is not a JSON-RPC message is answered `400` with the
//! Python SDK's JSON-RPC error (contracts/http/loopback.json).

#![allow(clippy::expect_used)]

use serde_json::json;
use wg_mcp::spec::not_jsonrpc_reply;

// covers: X6
#[test]
fn an_empty_object_gets_the_sdk_union_message() {
    let reply = not_jsonrpc_reply(b"{}").expect("not a JSON-RPC message");
    assert_eq!(reply["jsonrpc"], "2.0");
    assert_eq!(reply["id"], json!(null));
    assert_eq!(reply["error"]["code"], -32602);
    let message = reply["error"]["message"].as_str().expect("message");
    assert!(message.starts_with(
        "Validation error: 11 validation errors for union[JSONRPCRequest,JSONRPCNotification,JSONRPCResponse,JSONRPCError]\nJSONRPCRequest.jsonrpc\n  Field required [type=missing, input_value={}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.14/v/missing\nJSONRPCRequest.id\n"
    ));
    assert!(message.ends_with(
        "JSONRPCError.error\n  Field required [type=missing, input_value={}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.14/v/missing"
    ));
}

// covers: X6
#[test]
fn the_python_answers_to_non_jsonrpc_bodies_are_reproduced_byte_for_byte() {
    // tools/golden/x6_probe.py: the Python server's status and body text for each body.
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("golden/x6_bodies.json")).expect("golden json");
    let cases = golden["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 7);
    for case in cases {
        let body = case["body"].as_str().expect("body");
        let want = case["text"].as_str().expect("text");
        assert_eq!(case["status"], 400, "{}", case["case"]);
        let reply = not_jsonrpc_reply(body.as_bytes())
            .unwrap_or_else(|| panic!("{}: not refused", case["case"]));
        let got = serde_json::to_string(&reply).expect("encode");
        assert_eq!(got, want, "{}", case["case"]);
    }
}

// covers: X6
#[test]
fn a_complete_member_is_left_to_the_transport() {
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    assert_eq!(not_jsonrpc_reply(body), None);
}
