//! Stand-in for the `claude` CLI emitting documented `--output-format stream-json` messages (test
//! fixture; the Rust counterpart of `tests/fixtures/fake_claude.py`).
//!
//! `FAKE_CLAUDE_MODE` picks the behaviour: `success` (default), `error` (a failing result and
//! exit code 1) or `sleep` (never finishes). With `--version` it prints a version line. In the
//! `success` mode, a prompt containing `Task-ID: <KEY>` makes it commit a change with that
//! trailer in the current directory.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::process::{Command, ExitCode};

use serde_json::{Value, json};

fn out(message: &Value) {
    let mut stdout = std::io::stdout();
    writeln!(stdout, "{message}").unwrap();
    stdout.flush().unwrap();
}

fn git(args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .env("GIT_AUTHOR_NAME", "c")
        .env("GIT_AUTHOR_EMAIL", "c@x")
        .env("GIT_COMMITTER_NAME", "c")
        .env("GIT_COMMITTER_EMAIL", "c@x")
        .status()
        .unwrap();
    assert!(status.success());
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        println!("2.1.999 (Claude Code)");
        return ExitCode::SUCCESS;
    }
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).unwrap();
    let session = args
        .iter()
        .position(|a| a == "--session-id")
        .and_then(|i| args.get(i + 1))
        .map_or("resumed", String::as_str);
    let mode = std::env::var("FAKE_CLAUDE_MODE").unwrap_or_else(|_| "success".to_owned());
    out(&json!({
        "type": "system", "subtype": "init", "session_id": session,
        "model": "claude-test", "tools": ["Bash", "Edit"],
    }));
    out(&json!({"type": "assistant", "message": {"content": [
        {"type": "text", "text": "Working on it"},
        {"type": "tool_use", "id": "t1", "name": "Edit", "input": {}},
        {"type": "tool_use", "id": "t2", "name": "Task", "input": {}},
    ]}}));
    out(&json!({"type": "user", "message": {"content": [
        {"type": "tool_result", "tool_use_id": "t1", "is_error": true},
    ]}}));
    println!("a stray line that is not JSON");
    if mode == "sleep" {
        std::thread::sleep(std::time::Duration::from_secs(600));
    }
    if mode == "success"
        && let Some(rest) = prompt.split("Task-ID: ").nth(1)
    {
        let key = rest.split('`').next().unwrap_or("");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("claude_change.py")
            .unwrap();
        writeln!(file, "def from_claude():\n    return 1").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", &format!("claude work\n\nTask-ID: {key}")]);
    }
    let ok = mode == "success";
    out(&json!({
        "type": "result",
        "subtype": if ok { "success" } else { "error_during_execution" },
        "is_error": !ok, "duration_ms": 4321, "num_turns": 3, "result": "All done",
        "session_id": session, "total_cost_usd": 0.0123,
        "usage": {"input_tokens": 900, "output_tokens": 120, "cache_read_input_tokens": 50},
    }));
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
