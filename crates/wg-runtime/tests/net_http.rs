#![allow(clippy::expect_used, clippy::unwrap_used)]
//! The `Http` effect performed by the `wg-net` transport that `App::open` plugs into the
//! interpreter (WG-3741 item 2).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use wg_model::{Effect, EffectErrorKind, EffectOutcome, HttpCall, HttpMethod};
use wg_runtime::{Interpreter, NetHttp};

/// A server that answers one request with `201 Created`, a probe header and `body`.
fn serve_once(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let address = listener.local_addr().expect("local address");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut received = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !received.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => received.extend_from_slice(&chunk[..read]),
                }
            }
            let response = format!(
                "HTTP/1.1 201 Created\r\nX-Probe: yes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{address}/probe")
}

#[test]
fn http_effect_is_performed_by_the_net_transport() {
    let url = serve_once("hello");
    let mut interpreter = Interpreter {
        http: Some(Box::new(NetHttp::default())),
        ..Interpreter::default()
    };
    let call = HttpCall {
        method: HttpMethod::Get,
        url,
        headers: std::iter::empty().collect(),
        body: None,
        timeout_ms: Some(5_000),
    };

    let result = interpreter.perform(&Effect::Http(call));

    let EffectOutcome::Http(Ok(response)) = result.outcome else {
        panic!("expected a response, got {:?}", result.outcome);
    };
    assert_eq!(response.status, 201);
    assert_eq!(response.body, "hello");
    assert!(
        response
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("x-probe") && value == "yes")
    );
}

#[test]
fn http_effect_without_a_client_is_unavailable() {
    let mut interpreter = Interpreter::default();
    let call = HttpCall {
        method: HttpMethod::Get,
        url: "http://127.0.0.1:1/".to_owned(),
        headers: std::iter::empty().collect(),
        body: None,
        timeout_ms: None,
    };

    let result = interpreter.perform(&Effect::Http(call));

    let EffectOutcome::Http(Err(error)) = result.outcome else {
        panic!("expected an error, got {:?}", result.outcome);
    };
    assert_eq!(error.kind, EffectErrorKind::Unavailable);
}
