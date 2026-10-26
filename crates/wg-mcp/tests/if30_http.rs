//! IF-30: the Streamable HTTP endpoint answers like the stdio one and runs tools under the
//! `--grant` permissions (WG-3734). Binds an ephemeral port, never 8765.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wg_mcp::{Agent, ToolBackend, http_router};
use wg_mcpspec::options::ServeOptions;
use wg_types::{Permission, WgError};

/// Records the permissions each call ran under.
struct Recorder(Arc<Mutex<Vec<BTreeSet<Permission>>>>);

impl ToolBackend for Recorder {
    fn call_tool(
        &mut self,
        agent: &Agent,
        _name: &str,
        _arguments: Value,
    ) -> Result<Value, WgError> {
        self.0.lock().expect("lock").push(agent.permissions.clone());
        Ok(json!({"workspaces": []}))
    }

    fn read_resource(&mut self, _agent: &Agent, _uri: &str) -> Result<String, WgError> {
        Ok("[]".to_string())
    }
}

fn post(port: u16, session: Option<&str>, body: &Value) -> (String, String) {
    let body = body.to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let session = session
        .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /mcp/ HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-06-18\r\n\
         {session}Connection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read");
    let (head, text) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    (head.to_string(), text.to_string())
}

/// The JSON message in a reply that is either plain JSON or a server-sent event.
fn message(text: &str) -> Value {
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .find(|data| !data.is_empty());
    serde_json::from_str(data.unwrap_or(text.trim())).unwrap_or(Value::Null)
}

#[test]
fn http_serves_and_applies_grants() {
    let options = ServeOptions::parse(&[
        "--agent-name".to_string(),
        "http-test".to_string(),
        "--grant".to_string(),
        "run:start".to_string(),
    ])
    .expect("options");
    let ran = Arc::new(Mutex::new(Vec::new()));
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let cancel = CancellationToken::new();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    // The router alone: `wg-server` adds the token and Host/Origin checks in front of it.
    let router = http_router(
        options.agent(),
        Box::new(Recorder(Arc::clone(&ran))),
        cancel.clone(),
    )
    .expect("router");
    listener.set_nonblocking(true).expect("nonblocking");
    let server = runtime.spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
        axum::serve(listener, router).await
    });

    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "t", "version": "1"}}});
    let (head, text) = post(port, None, &init);
    let reply = message(&text);
    assert_eq!(
        reply["result"]["serverInfo"]["name"], "workgraph",
        "{head}\n{text}"
    );
    let session = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("mcp-session-id")
            .then(|| value.trim().to_string())
    });
    let session = session.as_deref();
    let _ = post(
        port,
        session,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );

    let (_, text) = post(
        port,
        session,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "workspace_list", "arguments": {}}}),
    );
    assert_eq!(message(&text)["result"]["isError"], false, "{text}");
    let ran = ran.lock().expect("lock");
    assert!(ran.iter().any(|p| p.contains(&Permission::RunStart)));

    let (_, text) = post(
        port,
        session,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "no_such_tool", "arguments": {}}}),
    );
    assert_eq!(
        message(&text)["result"]["content"][0]["text"],
        "Unknown tool: no_such_tool"
    );
    cancel.cancel();
    drop(server);
}
