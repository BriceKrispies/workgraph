//! The scripted `workgraph.agent.v1` agent.
//!
//! It performs real work: it edits files in its managed worktrees and creates Git commits
//! carrying a `Task-ID` trailer. If a repository contains `workgraph-demo.json`
//! (`{"file": ..., "content": ...}`) the agent writes that file; otherwise it appends a function
//! to the `--file` target.
//!
//! Modes: `success`, `buggy`, `fail`, `sleep`, `crash`, `noop`, `touch-ro`; any other value
//! behaves like `success`, as the Python agent did.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use wg_proc::{CaptureLimits, ProcError, ProcessSpec, base_environment_from, run_capture};

/// The default `--file` target.
pub const DEFAULT_FILE: &str = "agent_change.py";

const GIT_ENV: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Workgraph Scripted Agent"),
    ("GIT_AUTHOR_EMAIL", "agent@workgraph.local"),
    ("GIT_COMMITTER_NAME", "Workgraph Scripted Agent"),
    ("GIT_COMMITTER_EMAIL", "agent@workgraph.local"),
];

/// How long a single git invocation may run before it is terminated.
const GIT_TIMEOUT_MS: u64 = 120_000;

/// Parsed command line of the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentArgs {
    pub mode: String,
    pub file: String,
    pub manifest: PathBuf,
}

impl AgentArgs {
    /// Parse `--mode M`, `--file F` and the positional manifest path, in any order.
    ///
    /// # Errors
    /// A message for a missing manifest, an option without a value or an unknown option.
    pub fn parse(args: &[String]) -> std::result::Result<Self, String> {
        let mut mode = "success".to_owned();
        let mut file = DEFAULT_FILE.to_owned();
        let mut manifest = None;
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--mode" => {
                    mode = iter.next().ok_or("--mode needs a value")?.clone();
                }
                "--file" => {
                    file = iter.next().ok_or("--file needs a value")?.clone();
                }
                other if other.starts_with("--") => {
                    return Err(format!("unrecognized arguments: {other}"));
                }
                other => {
                    if manifest.is_some() {
                        return Err(format!("unrecognized arguments: {other}"));
                    }
                    manifest = Some(PathBuf::from(other));
                }
            }
        }
        let manifest = manifest.ok_or("the manifest path is required")?;
        Ok(Self {
            mode,
            file,
            manifest,
        })
    }
}

/// Failure of the agent: a missing or malformed input, a file error or a git error.
#[derive(Debug)]
pub enum AgentError {
    Message(String),
    Io(std::io::Error),
    Json(serde_json::Error),
    Process(ProcError),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Process(error) => write!(f, "{error}"),
        }
    }
}

impl From<std::io::Error> for AgentError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for AgentError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ProcError> for AgentError {
    fn from(error: ProcError) -> Self {
        Self::Process(error)
    }
}

type Result<T> = std::result::Result<T, AgentError>;

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

fn parse_object(text: &str) -> Result<Value> {
    let value: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(text)?
    };
    if value.is_object() {
        Ok(value)
    } else {
        Err(AgentError::Message("expected a JSON object".to_owned()))
    }
}

/// The string at a path of object keys; a missing key is an error, as a `KeyError` was in Python.
fn str_at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str> {
    let mut current = value;
    for key in path {
        current = current
            .get(key)
            .ok_or_else(|| AgentError::Message(format!("KeyError: {key:?}")))?;
    }
    current
        .as_str()
        .ok_or_else(|| AgentError::Message(format!("{path:?} is not a string")))
}

fn repositories(manifest: &Value) -> Result<&Vec<Value>> {
    manifest
        .get("repositories")
        .and_then(Value::as_array)
        .ok_or_else(|| AgentError::Message("KeyError: \"repositories\"".to_owned()))
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
    Ok(relative.display().to_string())
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
        return Err(AgentError::Message(format!(
            "git {} failed in {checkout}: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(())
}
