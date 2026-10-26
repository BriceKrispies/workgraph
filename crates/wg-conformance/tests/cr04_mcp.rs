//! CR-04: the stdio MCP client and the transcript replay, against a fake server.
//!
//! The fake server is this test binary run again with `WG_CR04_FAKE_SERVER` set (the target has
//! `harness = false` so that nothing else writes to its stdout). It answers `initialize`, echoes
//! the arguments of `echo`, reports `isError` for `fail`, and rejects other methods.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::process::ExitCode;
use wg_conformance::mcp::{
    PROTOCOL_VERSION, StdioClient, Verdict, canonical, parse_transcript, replay,
};

const FAKE_ENV: &str = "WG_CR04_FAKE_SERVER";

fn serve() {
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = req.get("id").cloned() else {
            continue;
        };
        let method = req
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let reply = match method {
            "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                "serverInfo": {"name": "fake", "version": "0"}}}),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match name {
                    "echo" => {
                        let text = canonical(params.get("arguments").unwrap_or(&Value::Null));
                        json!({"jsonrpc": "2.0", "id": id, "result": {
                            "content": [{"type": "text", "text": text}]}})
                    }
                    "fail" => json!({"jsonrpc": "2.0", "id": id, "result": {
                        "content": [{"type": "text", "text": "boom"}], "isError": true}}),
                    _ => json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32602, "message": "Invalid request parameters"}}),
                }
            }
            _ => json!({"jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": "Method not found"}}),
        };
        let mut out = stdout.lock();
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
}

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what.to_owned()) }
}

/// The program, its arguments and its environment for the fake server.
type Command = (String, Vec<String>, Vec<(String, String)>);

fn fake_command() -> Result<Command, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    Ok((
        exe.display().to_string(),
        Vec::new(),
        vec![(FAKE_ENV.to_owned(), "1".to_owned())],
    ))
}

fn start_fake() -> Result<StdioClient, String> {
    let (program, args, env) = fake_command()?;
    let dir = std::env::temp_dir();
    StdioClient::start(&program, &args, &env, &dir)
}

/// Requests, notifications and tool results travel over the pipe and come back as text.
fn round_trip() -> Result<(), String> {
    let mut client = start_fake()?;
    let init = client.initialize()?;
    let version = init
        .pointer("/result/protocolVersion")
        .and_then(Value::as_str);
    check(version == Some(PROTOCOL_VERSION), "initialize reply")?;
    let reply = client.call_tool("echo", json!({"b": 1, "a": {"y": 2, "x": 3}}))?;
    let text = reply
        .pointer("/result/content/0/text")
        .and_then(Value::as_str);
    check(
        text == Some(r#"{"a":{"x":3,"y":2},"b":1}"#),
        "echo keeps every argument",
    )?;
    let failed = client.call_tool("fail", json!({}))?;
    check(
        failed.pointer("/result/isError") == Some(&json!(true)),
        "isError is returned as a reply",
    )?;
    // Every sent message is recorded: initialize, its notification, and the two calls.
    let sent = client
        .transcript
        .iter()
        .filter(|m| m.direction == wg_conformance::mcp::Direction::Sent)
        .count();
    check(sent == 4, "every sent message is recorded")
}

/// A replay compares normalised replies: equal when the recorded reply matches, differs otherwise.
fn replay_verdicts() -> Result<(), String> {
    let transcript = [
        json!({"kind": "protocol", "label": "initialize", "seq": 1, "tool": null,
            "request": {"id": 1, "jsonrpc": "2.0", "method": "initialize", "params": {}},
            "response": {"id": 1, "jsonrpc": "2.0", "result": {
                "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                "serverInfo": {"name": "fake", "version": "0"}}}}),
        json!({"kind": "protocol", "label": "notifications/initialized", "seq": 2, "tool": null,
            "request": {"jsonrpc": "2.0", "method": "notifications/initialized"},
            "response": null}),
        json!({"kind": "ok", "label": "echo:ok", "seq": 3, "tool": "echo",
            "request": {"id": 2, "jsonrpc": "2.0", "method": "tools/call",
                "params": {"name": "echo", "arguments": {"id": "x"}}},
            "response": {"id": 2, "jsonrpc": "2.0", "result": {
                "content": [{"type": "text", "text": "{\"id\":\"x\"}"}]}}}),
        json!({"kind": "ok", "label": "echo:other", "seq": 4, "tool": "echo",
            "request": {"id": 3, "jsonrpc": "2.0", "method": "tools/call",
                "params": {"name": "echo", "arguments": {"id": "x"}}},
            "response": {"id": 3, "jsonrpc": "2.0", "result": {
                "content": [{"type": "text", "text": "{\"id\":\"y\"}"}]}}}),
    ];
    let text: String = transcript.iter().map(|v| format!("{v}\n")).collect();
    let exchanges = parse_transcript(&text)?;
    check(exchanges.len() == 4, "transcript has four exchanges")?;
    let mut client = start_fake()?;
    let rep = replay(&mut client, &exchanges, "fake");
    check(
        rep.verdicts
            == [
                Verdict::Equal,
                Verdict::NoReply,
                Verdict::Equal,
                Verdict::Differs,
            ],
        &format!("verdicts {:?}", rep.verdicts),
    )?;
    check(!rep.clean(), "a differing reply makes the replay unclean")
}

fn main() -> ExitCode {
    if std::env::var_os(FAKE_ENV).is_some() {
        serve();
        return ExitCode::SUCCESS;
    }
    type Test = fn() -> Result<(), String>;
    let tests: [(&str, Test); 2] = [
        ("round_trip", round_trip),
        ("replay_verdicts", replay_verdicts),
    ];
    let mut failed = 0;
    for (name, test) in tests {
        match test() {
            Ok(()) => println!("ok {name}"),
            Err(e) => {
                println!("FAILED {name}: {e}");
                failed += 1;
            }
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
