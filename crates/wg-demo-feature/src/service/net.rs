//! The subprocess steps of `demo e2e` (Shell): the MCP server over stdio (steps 7 and 8) and the
//! HTTP API of `workgraph serve` (steps 23 and 24). Python runs the first as a stdio client of
//! `python -m workgraph mcp` and the second as a uvicorn thread; here both are the current
//! executable, spawned as a child process over the same home.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wg_iface::cli::CliFailure;

use crate::spec::e2e::failed;

/// The longest the demonstration waits for one reply from a child process.
const REPLY_WINDOW: Duration = Duration::from_secs(60);

/// The protocol version the stdio client speaks.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// The current executable (`sys.executable -m workgraph` in Python).
fn workgraph() -> Result<std::path::PathBuf, CliFailure> {
    std::env::current_exe().map_err(|err| failed(format!("cannot locate workgraph: {err}")))
}

/// A stdio MCP client of `workgraph mcp --agent-name <name>` (`_mcp_session`).
pub struct McpSession {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl McpSession {
    /// Starts the server over `home` and runs the handshake.
    ///
    /// # Errors
    ///
    /// Returns a failure when the process or the handshake fails.
    pub fn start(home: &Path, agent_name: &str) -> Result<Self, CliFailure> {
        let mut child = Command::new(workgraph()?)
            .args(["mcp", "--agent-name", agent_name])
            .env("WORKGRAPH_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| failed(format!("cannot start the MCP server: {err}")))?;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| failed("the MCP server has no standard output"))?;
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut session = Self {
            child,
            stdin,
            lines,
            next_id: 0,
        };
        session.request(
            "initialize",
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                   "clientInfo": {"name": "workgraph-demo", "version": "1"}}),
        )?;
        session.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(session)
    }

    fn send(&mut self, body: &Value) -> Result<(), CliFailure> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| failed("the MCP server's input is closed"))?;
        writeln!(stdin, "{body}")
            .and_then(|()| stdin.flush())
            .map_err(|err| failed(format!("cannot write to the MCP server: {err}")))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, CliFailure> {
        self.next_id += 1;
        let id = json!(self.next_id);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let line = self
                .lines
                .recv_timeout(REPLY_WINDOW)
                .map_err(|_| failed(format!("the MCP server did not answer {method}")))?;
            let Ok(body) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if body.get("id") != Some(&id) {
                continue;
            }
            if let Some(error) = body.get("error") {
                return Err(failed(format!("MCP {method} failed: {error}")));
            }
            return Ok(body.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// The sorted names of the server's tools.
    ///
    /// # Errors
    ///
    /// Returns a failure when the request fails.
    pub fn tool_names(&mut self) -> Result<Vec<String>, CliFailure> {
        let result = self.request("tools/list", json!({}))?;
        let mut names: Vec<String> = result
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_owned))
            .collect();
        names.sort();
        Ok(names)
    }

    /// Calls tool `name`; the structured output, or the JSON text of its content.
    ///
    /// # Errors
    ///
    /// Returns a failure when the tool reports an error.
    pub fn call(&mut self, name: &str, arguments: &Value) -> Result<Value, CliFailure> {
        let result = self.request("tools/call", json!({"name": name, "arguments": arguments}))?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(failed(format!(
                "demonstration failed: MCP tool {name} failed: {}",
                result.get("content").unwrap_or(&Value::Null)
            )));
        }
        if let Some(structured) = result.get("structuredContent") {
            return Ok(structured.clone());
        }
        let text = result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("null");
        serde_json::from_str(text).map_err(|err| failed(format!("MCP {name}: {err}")))
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `workgraph serve` on a free loopback port, without the worker or the MCP mount (`_start_api`).
pub struct ApiServer {
    child: Child,
    pub base: String,
}

impl ApiServer {
    /// Starts the server over `home` and waits until its health route answers.
    ///
    /// # Errors
    ///
    /// Returns a failure when no port is free, the process cannot start, or it never answers.
    pub fn start(home: &Path) -> Result<Self, CliFailure> {
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .map_err(|err| failed(format!("no free port: {err}")))?
            .port();
        let child = Command::new(workgraph()?)
            .args(["serve", "--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["--no-worker", "--no-mcp"])
            .env("WORKGRAPH_HOME", home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| failed(format!("cannot start the API server: {err}")))?;
        let server = Self {
            child,
            base: format!("http://127.0.0.1:{port}"),
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if get(&format!("{}/api/v1/health", server.base), None)
                .is_ok_and(|(status, _)| status == 200)
            {
                return Ok(server);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(failed("the API server did not become healthy"))
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `GET url` with an optional bearer token: the status and the body text.
///
/// # Errors
///
/// Returns a failure when the request cannot be made.
pub fn get(url: &str, token: Option<&str>) -> Result<(u16, String), CliFailure> {
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .new_agent();
    let mut request = agent.get(url);
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request
        .call()
        .map_err(|err| failed(format!("GET {url} failed: {err}")))?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    Ok((status, body))
}

/// `GET url` as JSON.
///
/// # Errors
///
/// Returns a failure when the request fails or the body is not JSON.
pub fn get_json(url: &str, token: &str) -> Result<Value, CliFailure> {
    let (_, body) = get(url, Some(token))?;
    serde_json::from_str(&body).map_err(|err| failed(format!("GET {url}: {err}")))
}
