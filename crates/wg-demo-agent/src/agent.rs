//! The scripted `workgraph.agent.v1` agent: the I/O half. Parsing and errors live in
//! `wg-demoproto`.
//!
//! It performs real work: it edits files in its managed worktrees and creates Git commits
//! carrying a `Task-ID` trailer. If a repository contains `workgraph-demo.json`
//! (`{"file": ..., "content": ...}`) the agent writes that file; otherwise it appends a function
//! to the `--file` target.
//!
//! Modes: `success`, `buggy`, `fail`, `sleep`, `crash`, `noop`, `touch-ro`; any other value
//! behaves like `success`, as the Python agent did.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use wg_demoproto::{AgentArgs, Result, parse_object, repositories, str_at};
use wg_proc::{CaptureLimits, ProcessSpec, base_environment_from, run_capture};

const GIT_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Workgraph Scripted Agent"),
    ("GIT_AUTHOR_EMAIL", "agent@workgraph.local"),
    ("GIT_COMMITTER_NAME", "Workgraph Scripted Agent"),
    ("GIT_COMMITTER_EMAIL", "agent@workgraph.local"),
];

/// How long a single git invocation may run before it is terminated.
const GIT_TIMEOUT_MS: u64 = 120_000;

/// Run the agent for one request. Returns the process exit code.
///
/// # Errors
/// Malformed request or manifest, file errors and failed git commands; the caller reports them on
/// stderr and exits 1.
pub fn run(args: &AgentArgs, request_text: &str, out: &mut dyn Write) -> Result<i32> {
    let request = parse_object(request_text)?;
    let manifest = parse_object(&fs::read_to_string(&args.manifest)?)?;
    let task_key = str_at(&request, &["task", "key"])?.to_owned();
    emit(
        out,
        json!({"type": "progress", "message": format!("starting {task_key}"), "percent": 5}),
    )?;
    if args.mode == "sleep" {
        std::thread::sleep(Duration::from_secs(600));
    }
    if args.mode == "crash" {
        writeln!(out, "boom: simulated crash")?;
        return Ok(3);
    }
    if args.mode == "fail" {
        emit(
            out,
            json!({"type": "progress", "message": "tests are failing", "percent": 50}),
        )?;
        emit(
            out,
            json!({
                "type": "result",
                "status": "failed",
                "summary": "could not make it work",
                "error": "simulated failure",
            }),
        )?;
        return Ok(1);
    }
    let mut changed = Vec::new();
    for repo in repositories(&manifest)? {
        let access = str_at(repo, &["access"])?;
        if access != "read_write" && args.mode != "touch-ro" {
            continue;
        }
        if args.mode == "noop" {
            continue;
        }
        let name = str_at(repo, &["repository"])?;
        let path = implement(repo, &task_key, &args.mode, &args.file)?;
        changed.push(format!("{name}:{path}"));
        emit(
            out,
            json!({
                "type": "progress",
                "message": format!("committed {path} in {name}"),
                "percent": 60,
            }),
        )?;
    }
    emit(
        out,
        json!({"type": "usage", "input_tokens": 1200, "output_tokens": 340}),
    )?;
    let summary = if changed.is_empty() {
        "changed nothing".to_owned()
    } else {
        format!("changed {}", changed.join(", "))
    };
    emit(
        out,
        json!({"type": "result", "status": "succeeded", "summary": summary}),
    )?;
    Ok(0)
}

/// Write one protocol message as a JSON line.
fn emit(out: &mut dyn Write, message: Value) -> Result<()> {
    writeln!(out, "{}", serde_json::to_string(&message)?)?;
    out.flush()?;
    Ok(())
}

/// Edit one repository's checkout and commit the change. Returns the changed path, relative to
/// the checkout.
fn implement(repo: &Value, task_key: &str, mode: &str, default_file: &str) -> Result<String> {
    let checkout = str_at(repo, &["checkout_path"])?;
    let repository = str_at(repo, &["repository"])?;
    let root = Path::new(checkout);
    let plan_file = root.join("workgraph-demo.json");
    let relative = if plan_file.exists() {
        let plan = parse_object(&fs::read_to_string(&plan_file)?)?;
        let file = str_at(&plan, &["file"])?;
        let mut content = str_at(&plan, &["content"])?.replace("{task}", task_key);
        if mode == "buggy" {
            content = content.replace("WORKGRAPH_DEMO_OK", "TODO_FINISH");
        }
        let target = root.join(file);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, content)?;
        PathBuf::from(file)
    } else {
        let target = root.join(default_file);
        let previous = if target.exists() {
            fs::read_to_string(&target)?
        } else {
            String::new()
        };
        let index = previous.lines().count();
        let next = format!("{previous}def change_{index}():\n    return '{task_key}'\n");
        fs::write(&target, next)?;
        PathBuf::from(default_file)
    };
    git(checkout, &["add", "-A"])?;
    let message = format!("Implement {task_key} in {repository}\n\nTask-ID: {task_key}");
    git(checkout, &["commit", "-q", "-m", &message])?;
    // `pathlib` renders the path with the platform separator.
    Ok(relative
        .components()
        .collect::<PathBuf>()
        .display()
        .to_string())
}

/// Run one git command in `checkout` with the agent's author identity.
fn git(checkout: &str, args: &[&str]) -> Result<()> {
    let mut argv = vec!["git".to_owned(), "-C".to_owned(), checkout.to_owned()];
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    let mut spec = ProcessSpec::new(argv);
    spec.env = Some(base_environment_from(std::env::vars_os(), GIT_ENV, true));
    spec.timeout_ms = Some(GIT_TIMEOUT_MS);
    let result = run_capture(&spec, CaptureLimits::default(), None)?;
    if result.timed_out || result.exit_code != Some(0) {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(wg_demoproto::AgentError::Message(format!(
            "git {} failed in {checkout}: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(())
}
