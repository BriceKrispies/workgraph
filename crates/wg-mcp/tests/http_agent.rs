//! WG-5702: a call over Streamable HTTP with the `X-Workgraph-Agent` header is attributed to the
//! named agent exactly as the same call over stdio with `--agent-name` is, and a call without the
//! header keeps the agent the flags set.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wg_iface::mcp::options::ServeOptions;
use wg_mcp::{Agent, ToolBackend, http_router};
use wg_types::WgError;

/// Records the agent name of every call and answers with an empty structured result.
struct Recorder {
    seen: Arc<Mutex<Vec<String>>>,
}

impl ToolBackend for Recorder {
    fn call_tool(
        &mut self,
        agent: &Agent,
        _name: &str,
        _arguments: Value,
    ) -> Result<Value, WgError> {
        self.seen.lock().unwrap().push(agent.name.clone());
        Ok(json!({}))
    }

    fn read_resource(&mut self, _agent: &Agent, uri: &str) -> Result<String, WgError> {
        Err(WgError::not_found(uri))
    }
}

/// Serves `agent` over HTTP on a free loopback port; returns the endpoint URL.
fn serve(agent: Agent, seen: Arc<Mutex<Vec<String>>>) -> String {
    let router =
        http_router(agent, Box::new(Recorder { seen }), CancellationToken::new()).expect("router");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("bind");
    let address = listener.local_addr().expect("address");
    std::thread::spawn(move || {
        let _ = runtime.block_on(async { axum::serve(listener, router).await });
    });
    format!("http://{address}/mcp/")
}

/// One JSON-RPC POST. Returns the response's session header, when it has one.
fn post(
    client: &ureq::Agent,
    url: &str,
    body: &Value,
    session: Option<&str>,
    agent_header: Option<&str>,
) -> Option<String> {
    let mut request = client
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .header("MCP-Protocol-Version", "2025-06-18");
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    if let Some(agent) = agent_header {
        request = request.header("X-Workgraph-Agent", agent);
    }
    let mut response = request.send(body.to_string()).expect("the request is sent");
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    // The answer is an event stream: reading it to its end waits for the call to have run.
    let _ = response.body_mut().read_to_string();
    session
}

/// Opens a session, then makes one tool call with `agent_header` (if any).
fn call_over_http(url: &str, agent_header: Option<&str>) {
    let client: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }
    });
    let session = post(&client, url, &initialize, None, None).expect("a session id");
    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    post(&client, url, &initialized, Some(&session), None);
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "workspace_list", "arguments": {}}
    });
    post(&client, url, &call, Some(&session), agent_header);
}

#[test]
fn http_header_attributes_calls_as_stdio_agent_name_does() {
    let flags = ServeOptions::parse(&["--agent-name".to_owned(), "claude-code".to_owned()])
        .expect("the flags parse")
        .agent();
    // Over stdio the call runs under the flags' agent unchanged.
    let stdio_name = flags.name.clone();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let url = serve(
        ServeOptions::parse(&["--agent-name".to_owned(), "mcp-client".to_owned()])
            .expect("the flags parse")
            .agent(),
        Arc::clone(&seen),
    );
    call_over_http(&url, Some("claude-code"));
    call_over_http(&url, None);

    let recorded = seen.lock().unwrap().clone();
    assert_eq!(recorded, vec![stdio_name, "mcp-client".to_owned()]);
}
