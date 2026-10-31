//! CR-05: the HTTP client sends requests, decodes chunked bodies and captures server-sent
//! events up to `max_events`, against a local fake server. No Workgraph process is started.

use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use wg_conformance::http::{Request, send};

fn expect<T>(r: Result<T, String>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => panic!("{e}"),
    }
}

/// Serve one canned response per connection. The server keeps each connection open for a
/// while after writing, so a client that waits for end of stream would stall.
fn fake_server(responses: Vec<Vec<u8>>) -> u16 {
    let listener = expect(TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string()));
    let port = expect(listener.local_addr().map_err(|e| e.to_string())).port();
    thread::spawn(move || {
        for response in responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0_u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(&response);
            thread::sleep(Duration::from_secs(5));
        }
    });
    port
}

fn chunked(parts: &[&str]) -> Vec<u8> {
    let mut out = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\n\r\n",
    );
    for part in parts {
        out.push_str(&format!("{:x}\r\n{part}\r\n", part.len()));
    }
    out.into_bytes()
}

#[test]
fn json_body_with_content_length() {
    let body = r#"{"status":"ok"}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let port = fake_server(vec![response.into_bytes()]);
    let req = Request {
        method: "GET",
        path: "/api/v1/health",
        headers: &[],
        body: None,
    };
    let ex = expect(send(port, &req, None));
    assert_eq!(ex.status, 200);
    assert_eq!(ex.header("content-type"), Some("application/json"));
    assert_eq!(ex.body, body.as_bytes());
    assert!(ex.events.is_empty());
}

#[test]
fn sse_capture_stops_at_max_events_without_end_of_stream() {
    let port = fake_server(vec![chunked(&[
        "id: 1\r\nevent: domain\r\ndata: {\"seq\":1}\r\n\r\n",
        "event: queue\r\ndata: {\"queued\": 2}\r\n\r\n",
        "data: 3\r\n\r\n",
    ])]);
    let req = Request {
        method: "GET",
        path: "/api/v1/ws/default/events/stream?after_seq=0",
        headers: &[],
        body: None,
    };
    let ex = expect(send(port, &req, Some(2)));
    assert_eq!(ex.status, 200);
    assert_eq!(
        ex.events,
        vec![
            json!({"event": "domain", "data": {"seq": 1}, "id": "1"}),
            json!({"event": "queue", "data": {"queued": 2}}),
        ]
    );
}

#[test]
fn chunked_json_body_ends_at_the_zero_chunk() {
    let body = r#"{"a":1}"#;
    let mut response = String::from(
        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n",
    );
    response.push_str(&format!("{:x}\r\n{body}\r\n0\r\n\r\n", body.len()));
    let port = fake_server(vec![response.into_bytes()]);
    let headers = [("content-type".to_owned(), "application/json".to_owned())];
    let req = Request {
        method: "POST",
        path: "/x",
        headers: &headers,
        body: Some(b"{}"),
    };
    let ex = expect(send(port, &req, None));
    assert_eq!(ex.status, 201);
    assert_eq!(ex.body, body.as_bytes());
}
