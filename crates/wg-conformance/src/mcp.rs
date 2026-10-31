//! MCP over stdio (CR-04): a JSON-RPC 2.0 client for `mcp` scenario steps, the raw message
//! capture every exchange goes through, and the CF-03 replay command.
//!
//! The client writes one JSON document per line to the server's stdin and reads one per line from
//! its stdout (the MCP stdio transport). A reader thread turns stdout into a channel so a silent
//! server is a timeout, not a hang. Every message in either direction is kept verbatim.
//!
//! The replay command sends each recorded request of `contracts/mcp/stdio.jsonl` unchanged and
//! compares the reply with the recorded one after the section 3.4 normaliser, which rewrites the
//! ids and timestamps that differ between two runs. The HTTP transcript is replayed by CR-05.

use crate::compare::Normaliser;
use crate::normalise::FullNormaliser;
use crate::runner::{Outcome, RunResult, Status, StepRecord};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};

/// The protocol version the recorder negotiated (`contracts/mcp/stdio.jsonl`).
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Which way a message travelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Sent,
    Received,
}

/// One JSON-RPC message as it crossed the pipe.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub direction: Direction,
    pub body: Value,
}

/// The arguments that start a stdio server after `prefix` (the implementation's own arguments):
/// `mcp --agent-name conformance`, then one `--grant` per permission.
pub fn mcp_args(prefix: &[String], grants: &[String]) -> Vec<String> {
    // Grants are passed after the agent identity, one `--grant` each (see `mcp --help`).
    let mut args = prefix.to_vec();
    args.extend(["mcp", "--agent-name", "conformance"].map(str::to_owned));
    for g in grants {
        args.push("--grant".to_owned());
        args.push(g.clone());
    }
    args
}

/// A stdio MCP server process and the channel its stdout arrives on.
pub struct StdioClient {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<std::io::Result<String>>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
    initialized: bool,
    /// Every message sent or received, in order.
    pub transcript: Vec<Message>,
}

impl StdioClient {
    /// Start `program args...` (build `args` with [`mcp_args`]) with `env` in working directory `dir`.
    pub fn start(
        program: &str,
        args: &[String],
        env: &[(String, String)],
        dir: &Path,
    ) -> Result<Self, String> {
        let mut child = Command::new(program)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {program}: {e}"))?;
        let stdin = child.stdin.take();
        let Some(stdout) = child.stdout.take() else {
            return Err("server stdout was not piped".to_owned());
        };
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(pipe) = child.stderr.take() {
            let sink = Arc::clone(&stderr);
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if let Ok(mut text) = sink.lock() {
                        text.push_str(&line);
                        text.push('\n');
                    }
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            lines,
            stderr,
            next_id: 0,
            initialized: false,
            transcript: Vec::new(),
        })
    }

    /// Write one message as a line and record it.
    pub fn send(&mut self, body: &Value) -> Result<(), String> {
        let line = serde_json::to_string(body).map_err(|e| format!("encode: {e}"))?;
        let note = self.stderr_note();
        let Some(stdin) = self.stdin.as_mut() else {
            return Err("server stdin is closed".to_owned());
        };
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("write to server: {e}{note}"))?;
        self.transcript.push(Message {
            direction: Direction::Sent,
            body: body.clone(),
        });
        Ok(())
    }

    /// Send a notification (no id, no reply expected).
    pub fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut body = json!({"jsonrpc": "2.0", "method": method});
        if let Some(p) = params {
            body["params"] = p;
        }
        self.send(&body)
    }

    /// Send a request with a fresh id and return the reply object (with `result` or `error`).
    pub fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        self.next_id += 1;
        let id = json!(self.next_id);
        let mut body = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if let Some(p) = params {
            body["params"] = p;
        }
        self.send(&body)?;
        self.await_response(&id)
    }

    /// Read messages until the reply with `id` arrives. Notifications and other messages that come
    /// first are recorded and skipped.
    pub fn await_response(&mut self, id: &Value) -> Result<Value, String> {
        // Polled in short slices so a spent budget is seen at once; the stall window is still
        // measured from the last message received.
        let mut quiet = std::time::Instant::now();
        loop {
            if crate::budget::cut() {
                return Err(crate::budget::cut_error());
            }
            let window = crate::wait::stall_timeout();
            let line = match self.lines.recv_timeout(crate::wait::POLL) {
                Ok(Ok(line)) => {
                    quiet = std::time::Instant::now();
                    line
                }
                Ok(Err(e)) => return Err(format!("read from server: {e}")),
                Err(RecvTimeoutError::Timeout) if quiet.elapsed() < window => continue,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "stalled: no reply for {:.1} s{}",
                        window.as_secs_f64(),
                        self.stderr_note()
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(format!("server closed its stdout{}", self.stderr_note()));
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let body: Value = serde_json::from_str(&line)
                .map_err(|e| format!("invalid JSON from server: {e}: {line}"))?;
            let is_reply = body.get("result").is_some() || body.get("error").is_some();
            let matches = is_reply && body.get("id") == Some(id);
            self.transcript.push(Message {
                direction: Direction::Received,
                body: body.clone(),
            });
            if matches {
                return Ok(body);
            }
        }
    }

    /// The MCP handshake: `initialize`, then `notifications/initialized`. Runs once.
    pub fn initialize(&mut self) -> Result<Value, String> {
        if self.initialized {
            return Ok(Value::Null);
        }
        let reply = self.request(
            "initialize",
            Some(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "wg-conformance", "version": "1"},
            })),
        )?;
        self.notify("notifications/initialized", None)?;
        self.initialized = true;
        Ok(reply)
    }

    /// `tools/call` for one tool; the reply object is returned as the server sent it.
    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        self.initialize()?;
        self.request(
            "tools/call",
            Some(json!({"name": name, "arguments": arguments})),
        )
    }

    /// `resources/read` for one URI; the reply object is returned as the server sent it.
    pub fn read_resource(&mut self, uri: &str) -> Result<Value, String> {
        self.initialize()?;
        self.request("resources/read", Some(json!({"uri": uri})))
    }

    fn stderr_note(&self) -> String {
        let text = self
            .stderr
            .lock()
            .map(|t| t.trim().to_owned())
            .unwrap_or_default();
        if text.is_empty() {
            String::new()
        } else {
            format!("; server stderr: {text}")
        }
    }
}

impl Drop for StdioClient {
    fn drop(&mut self) {
        // Closing stdin is the MCP shutdown signal; kill the process only if it lingers.
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Run one `mcp` scenario step. The session starts on the first such step and lasts for the
/// scenario. The step passes when the server replies. A tool error (`isError`, or a JSON-RPC
/// error) is a normal reply: it is returned as exit `1` so that later `assert` steps see it the way
/// they see a failed `cli` call. Only a transport failure fails the step.
pub(crate) fn tool_step(
    session: &mut Option<StdioClient>,
    start: impl FnOnce() -> Result<StdioClient, String>,
    mut rec: StepRecord,
    tool: &str,
    args: &Value,
) -> (StepRecord, Option<Outcome>) {
    if session.is_none() {
        match start() {
            Ok(client) => *session = Some(client),
            Err(e) => return (fail(rec, e), None),
        }
    }
    let Some(client) = session.as_mut() else {
        return (fail(rec, "no MCP session".to_owned()), None);
    };
    let reply = match client.call_tool(tool, args.clone()) {
        Ok(reply) => reply,
        Err(e) => return (fail(rec, e), None),
    };
    let outcome = outcome_of(&reply);
    rec.exit = Some(outcome.exit);
    rec.stdout.clone_from(&outcome.stdout);
    rec.stderr.clone_from(&outcome.stderr);
    rec.status = Status::Passed;
    (rec, Some(outcome))
}

/// The `cli`-shaped view of a `tools/call` reply: the text content as stdout, errors as stderr.
fn outcome_of(reply: &Value) -> Outcome {
    if let Some(error) = reply.get("error") {
        return Outcome {
            exit: 1,
            stdout: String::new(),
            stderr: error.to_string(),
        };
    }
    let result = reply.get("result").cloned().unwrap_or(Value::Null);
    let text: String = result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_error {
        Outcome {
            exit: 1,
            stdout: String::new(),
            stderr: text,
        }
    } else {
        Outcome {
            exit: 0,
            stdout: text,
            stderr: String::new(),
        }
    }
}

/// Run one `mcp_resource` scenario step: `resources/read` on the shared session. The first text
/// content is the stdout; an error reply is exit `1` with the error as stderr.
pub(crate) fn resource_step(
    session: &mut Option<StdioClient>,
    start: impl FnOnce() -> Result<StdioClient, String>,
    mut rec: StepRecord,
    uri: &str,
) -> (StepRecord, Option<Outcome>) {
    if session.is_none() {
        match start() {
            Ok(client) => *session = Some(client),
            Err(e) => return (fail(rec, e), None),
        }
    }
    let Some(client) = session.as_mut() else {
        return (fail(rec, "no MCP session".to_owned()), None);
    };
    let reply = match client.read_resource(uri) {
        Ok(reply) => reply,
        Err(e) => return (fail(rec, e), None),
    };
    let outcome = match reply.get("error") {
        Some(error) => Outcome {
            exit: 1,
            stdout: String::new(),
            stderr: error.to_string(),
        },
        None => Outcome {
            exit: 0,
            stdout: reply
                .pointer("/result/contents/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            stderr: String::new(),
        },
    };
    rec.exit = Some(outcome.exit);
    rec.stdout.clone_from(&outcome.stdout);
    rec.stderr.clone_from(&outcome.stderr);
    rec.status = Status::Passed;
    (rec, Some(outcome))
}

fn fail(rec: StepRecord, note: String) -> StepRecord {
    StepRecord {
        status: Status::Failed,
        note,
        ..rec
    }
}

/// One exchange of a transcript file.
#[derive(Debug, Clone)]
pub struct Exchange {
    pub seq: u64,
    pub kind: String,
    pub label: String,
    pub request: Value,
    /// The recorded reply; `None` for notifications.
    pub response: Option<Value>,
}

/// Parse a JSON-lines transcript (one exchange per line, as `record_mcp.py` writes it).
pub fn parse_transcript(text: &str) -> Result<Vec<Exchange>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        let text_of = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let request = v
            .get("request")
            .cloned()
            .ok_or_else(|| format!("line {}: no request", i + 1))?;
        let response = v.get("response").filter(|r| !r.is_null()).cloned();
        out.push(Exchange {
            seq: v.get("seq").and_then(Value::as_u64).unwrap_or(i as u64 + 1),
            kind: text_of("kind"),
            label: text_of("label"),
            request,
            response,
        });
    }
    Ok(out)
}

/// The verdict for one replayed exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The request was a notification; nothing to compare.
    NoReply,
    Equal,
    Differs,
    /// The server gave no reply (timeout, closed pipe, invalid JSON).
    Missing,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::NoReply => "notification",
            Verdict::Equal => "equal",
            Verdict::Differs => "differs",
            Verdict::Missing => "missing",
        }
    }
}

/// The replay of a whole transcript against one implementation.
#[derive(Debug, Clone)]
pub struct Replay {
    pub seqs: Vec<u64>,
    pub labels: Vec<String>,
    pub verdicts: Vec<Verdict>,
    /// Normalised recorded and observed reply per exchange (`None` when absent).
    pub expected: Vec<Option<String>>,
    pub actual: Vec<Option<String>>,
    pub errors: Vec<Option<String>>,
}

impl Replay {
    pub fn count(&self, v: Verdict) -> usize {
        self.verdicts.iter().filter(|x| **x == v).count()
    }

    /// The replay is clean when every compared reply equals the recorded one.
    pub fn clean(&self) -> bool {
        self.count(Verdict::Differs) == 0 && self.count(Verdict::Missing) == 0
    }
}

/// JSON with object keys sorted at every level, so equal documents give equal text whatever the key
/// order of the server or the `preserve_order` setting of serde_json.
pub fn canonical(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .filter_map(|k| {
                    map.get(k)
                        .map(|child| format!("{}:{}", Value::String(k.clone()), canonical(child)))
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

/// Send every exchange of `transcript` to `client` in order and compare the replies.
///
/// Recorded and observed replies are canonicalised and normalised together, so one value gets one
/// placeholder on both sides, and a reply equals the recorded one when the normalised texts match.
pub fn replay(client: &mut StdioClient, transcript: &[Exchange], implementation: &str) -> Replay {
    // Per exchange: the observed reply (None for notifications and missing replies) and the error.
    let mut observed: Vec<(Option<Value>, Option<String>)> = Vec::new();
    for ex in transcript {
        match ex.request.get("id").cloned() {
            None => {
                let error = client.send(&ex.request).err();
                observed.push((None, error));
            }
            Some(id) => {
                let reply = client
                    .send(&ex.request)
                    .and_then(|()| client.await_response(&id));
                match reply {
                    Ok(actual) => observed.push((Some(actual), None)),
                    Err(e) => observed.push((None, Some(e))),
                }
            }
        }
    }
    let n = transcript.len();
    let mut text = String::new();
    for ex in transcript {
        text.push_str(&ex.response.as_ref().map(canonical).unwrap_or_default());
        text.push('\n');
    }
    for (actual, _) in &observed {
        text.push_str(&actual.as_ref().map(canonical).unwrap_or_default());
        text.push('\n');
    }
    let run = RunResult {
        scenario: "mcp-replay".to_owned(),
        implementation: implementation.to_owned(),
        roots: Vec::new(),
        steps: Vec::new(),
    };
    let normalised = FullNormaliser::default().normalise(&run, &text);
    let lines: Vec<&str> = normalised.lines().collect();
    let line = |at: usize| {
        lines
            .get(at)
            .filter(|s| !s.is_empty())
            .map(|s| (*s).to_owned())
    };
    let mut rep = Replay {
        seqs: Vec::new(),
        labels: Vec::new(),
        verdicts: Vec::new(),
        expected: Vec::new(),
        actual: Vec::new(),
        errors: Vec::new(),
    };
    for (i, ex) in transcript.iter().enumerate() {
        let (expected, actual) = (line(i), line(n + i));
        let verdict = if ex.request.get("id").is_none() {
            Verdict::NoReply
        } else if actual.is_none() {
            Verdict::Missing
        } else if expected.is_some() && expected == actual {
            Verdict::Equal
        } else {
            Verdict::Differs
        };
        rep.seqs.push(ex.seq);
        rep.labels.push(ex.label.clone());
        rep.verdicts.push(verdict);
        rep.expected.push(expected);
        rep.actual.push(actual);
        rep.errors.push(observed[i].1.clone());
    }
    rep
}

/// Summary text for a replay: one line per exchange and totals.
pub fn replay_text(rep: &Replay) -> String {
    let mut out = String::new();
    for (i, v) in rep.verdicts.iter().enumerate() {
        out.push_str(&format!(
            "{:<12} {:>4} {}\n",
            v.as_str(),
            rep.seqs[i],
            rep.labels[i]
        ));
        if let Some(e) = &rep.errors[i] {
            out.push_str(&format!("             error: {e}\n"));
        }
    }
    out.push_str(&format!(
        "{} exchanges: {} equal, {} differ, {} missing, {} notifications\n",
        rep.verdicts.len(),
        rep.count(Verdict::Equal),
        rep.count(Verdict::Differs),
        rep.count(Verdict::Missing),
        rep.count(Verdict::NoReply),
    ));
    out
}

/// Replay report as JSON: totals and per-exchange verdicts with the normalised replies.
pub fn replay_json(rep: &Replay) -> Value {
    let rows: Vec<Value> = rep
        .verdicts
        .iter()
        .enumerate()
        .map(|(i, v)| {
            json!({
                "seq": rep.seqs[i], "label": rep.labels[i], "verdict": v.as_str(),
                "expected": rep.expected[i], "actual": rep.actual[i], "error": rep.errors[i],
            })
        })
        .collect();
    json!({
        "equal": rep.count(Verdict::Equal),
        "differ": rep.count(Verdict::Differs),
        "missing": rep.count(Verdict::Missing),
        "notifications": rep.count(Verdict::NoReply),
        "exchanges": rows,
    })
}

const REPLAY_USAGE: &str = "usage: wg-conformance mcp-replay --command COMMAND [options]
  --command COMMAND    implementation command line, e.g. `workgraph` (required)
  --transcript FILE    JSON-lines transcript (default contracts/mcp/stdio.jsonl)
  --grant PERMISSION   grant passed to `mcp` (repeatable)
  --report FILE        write the JSON report
exit status: 0 when every compared reply equals the recorded one, 1 otherwise, 2 on usage errors";

/// `wg-conformance mcp-replay`: replay a stdio transcript (CF-03) against an implementation.
pub fn replay_command(args: &[String]) -> ExitCode {
    let mut command = None;
    let mut transcript = PathBuf::from("contracts/mcp/stdio.jsonl");
    let mut report = None;
    let mut grants: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let Some(value) = it.next() else {
            eprintln!("error: {flag} needs a value\n{REPLAY_USAGE}");
            return ExitCode::from(2);
        };
        match flag.as_str() {
            "--command" => command = Some(value.clone()),
            "--transcript" => transcript = PathBuf::from(value),
            "--report" => report = Some(PathBuf::from(value)),
            "--grant" => grants.push(value.clone()),
            other => {
                eprintln!("error: unknown option `{other}`\n{REPLAY_USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(command) = command else {
        eprintln!("error: mcp-replay needs --command\n{REPLAY_USAGE}");
        return ExitCode::from(2);
    };
    match run_replay(&command, &transcript, &grants, report.as_deref()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run_replay(
    command: &str,
    transcript: &Path,
    grants: &[String],
    report: Option<&Path>,
) -> Result<bool, String> {
    let text = std::fs::read_to_string(transcript)
        .map_err(|e| format!("read {}: {e}", transcript.display()))?;
    let exchanges = parse_transcript(&text)?;
    let mut parts = command.split_whitespace().map(str::to_owned);
    let program = parts.next().ok_or("empty --command")?;
    let args = mcp_args(&parts.collect::<Vec<_>>(), grants);
    let root = std::env::temp_dir().join(format!("wgconf-mcp-replay-{}", std::process::id()));
    let (home, tmp) = (root.join("home"), root.join("tmp"));
    for d in [&home, &tmp] {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    let env = vec![
        ("WORKGRAPH_HOME".to_owned(), home.display().to_string()),
        ("COLUMNS".to_owned(), "120".to_owned()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("TMPDIR".to_owned(), tmp.display().to_string()),
        ("TMP".to_owned(), tmp.display().to_string()),
        ("TEMP".to_owned(), tmp.display().to_string()),
    ];
    let result = StdioClient::start(&program, &args, &env, &tmp)
        .map(|mut client| replay(&mut client, &exchanges, command));
    let _ = std::fs::remove_dir_all(&root);
    let rep = result?;
    print!("{}", replay_text(&rep));
    if let Some(path) = report {
        let mut out = serde_json::to_string_pretty(&replay_json(&rep)).unwrap_or_default();
        out.push('\n');
        std::fs::write(path, out).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(rep.clean())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_stdio_transcript_parses() -> Result<(), String> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../contracts/mcp/stdio.jsonl"
        );
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let exchanges = parse_transcript(&text)?;
        // Every recorded line is one exchange; the count changes whenever the transcript is re-recorded.
        let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
        assert_eq!(exchanges.len(), lines);
        assert_eq!(exchanges[0].label, "initialize");
        assert!(exchanges[0].response.is_some());
        assert!(
            exchanges[1].response.is_none(),
            "notifications have no reply"
        );
        Ok(())
    }

    #[test]
    fn canonical_ignores_key_order() -> Result<(), String> {
        let a: Value = serde_json::from_str(r#"{"b": [1, {"y": 2, "x": 1}], "a": "s"}"#)
            .map_err(|e| e.to_string())?;
        let b: Value = serde_json::from_str(r#"{"a": "s", "b": [1, {"x": 1, "y": 2}]}"#)
            .map_err(|e| e.to_string())?;
        assert_eq!(canonical(&a), canonical(&b));
        assert_eq!(canonical(&a), r#"{"a":"s","b":[1,{"x":1,"y":2}]}"#);
        Ok(())
    }

    #[test]
    fn tool_reply_maps_text_and_errors() {
        let ok = json!({"id": 1, "result": {"content": [{"type": "text", "text": "{\"a\": 1}"}]}});
        let out = outcome_of(&ok);
        assert_eq!((out.exit, out.stdout.as_str()), (0, "{\"a\": 1}"));
        let tool_error = json!({"id": 2, "result": {"content": [{"type": "text", "text": "Unknown tool"}], "isError": true}});
        let out = outcome_of(&tool_error);
        assert_eq!((out.exit, out.stderr.as_str()), (1, "Unknown tool"));
        let rpc_error = json!({"id": 3, "error": {"code": -32602, "message": "bad"}});
        assert_eq!(outcome_of(&rpc_error).exit, 1);
    }
}
