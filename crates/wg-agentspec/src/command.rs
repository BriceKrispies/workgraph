//! The generic command agent adapter (P04, the `workgraph.agent.v1` JSON-lines protocol), pure
//! part.
//!
//! Workgraph runs an explicitly configured executable with an argument template. No shell is
//! involved: every template element becomes exactly one argument after placeholder substitution.
//! Allowed placeholders: `{attempt_id} {task_id} {task_key} {workdir} {manifest} {prompt_file}`.
//!
//! Output is one JSON object per stdout line with `type` one of `progress`, `log`, `usage`,
//! `artifact` or `result`. Non-JSON lines are kept as log output. Exit code 0 without a `result`
//! line is a success with an empty summary; any other exit code is a failure.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::{Map, Value, json};
use wg_procspec::ProcessResult;

use crate::error::AgentError;
use crate::types::{
    AgentCapabilities, AgentEvent, AgentRunRequest, AgentRunResult, EventKind, RunStatus,
};
use crate::util::{as_int, as_number, py_str};
use wg_types::pyval::{get, prefix, text, truthy};

/// The protocol name in the request payload.
pub const PROTOCOL: &str = "workgraph.agent.v1";

/// The placeholders an argument template may use.
pub const PLACEHOLDERS: [&str; 6] = [
    "attempt_id",
    "task_id",
    "task_key",
    "workdir",
    "manifest",
    "prompt_file",
];

/// `CommandAgentConfig`.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandConfig {
    pub executable: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout_seconds: f64,
    pub supports_progress: bool,
    pub supports_usage: bool,
    pub supports_resume: bool,
}

impl CommandConfig {
    /// A config for `executable` with the defaults of the Python model.
    pub fn new(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            env: Vec::new(),
            timeout_seconds: 1800.0,
            supports_progress: true,
            supports_usage: false,
            supports_resume: false,
        }
    }
}

impl CommandConfig {
    /// The `CommandAgentConfig` of a dumped `agents` entry. The executable is required by the
    /// model; the other settings take its defaults when absent.
    #[must_use]
    pub fn from_config(entry: &serde_json::Map<String, serde_json::Value>) -> Self {
        let flag = |key: &str, default: bool| {
            entry
                .get(key)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(default)
        };
        let defaults = Self::new(String::new());
        let mut config = Self::new(text(entry.get("executable")));
        config.args = entry
            .get("args")
            .and_then(serde_json::Value::as_array)
            .map_or_else(Vec::new, |items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            });
        config.env = entry
            .get("env")
            .and_then(serde_json::Value::as_object)
            .map_or_else(Vec::new, |env| {
                env.iter()
                    .filter_map(|(key, value)| {
                        value.as_str().map(|value| (key.clone(), value.to_owned()))
                    })
                    .collect()
            });
        config.timeout_seconds = entry
            .get("timeout_seconds")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(defaults.timeout_seconds);
        config.supports_progress = flag("supports_progress", defaults.supports_progress);
        config.supports_usage = flag("supports_usage", defaults.supports_usage);
        config.supports_resume = flag("supports_resume", defaults.supports_resume);
        config
    }
}

/// The command agent provider: a name and its configuration. Running it is `wg-agents`.
#[derive(Debug, Clone)]
pub struct CommandAgentProvider {
    pub name: String,
    pub config: CommandConfig,
}

impl CommandAgentProvider {
    /// The adapter type name.
    pub const ADAPTER_TYPE: &'static str = "command";

    /// A provider named `name`.
    ///
    /// # Errors
    /// [`AgentError::Config`] when the argument template uses an unknown placeholder.
    pub fn new(name: impl Into<String>, config: CommandConfig) -> Result<Self, AgentError> {
        validate_template(&config.args).map_err(AgentError::Config)?;
        Ok(Self {
            name: name.into(),
            config,
        })
    }

    /// What this agent declares it can do.
    pub fn capabilities(&self) -> AgentCapabilities {
        command_capabilities(&self.config)
    }

    /// The non-secret configuration recorded on each attempt.
    pub fn config_snapshot(&self) -> Value {
        command_config_snapshot(&self.config)
    }
}

/// An argument template names a placeholder that does not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownPlaceholder {
    pub name: String,
}

impl fmt::Display for UnknownPlaceholder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown placeholder {{{}}} in agent argument template (allowed: {})",
            self.name,
            PLACEHOLDERS
                .iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl std::error::Error for UnknownPlaceholder {}

/// A piece of a template argument.
enum Piece<'a> {
    Text(&'a str),
    Placeholder(&'a str),
}

/// Split `arg` the way the regex `\{([^{}]*)\}` finds matches: leftmost, non-overlapping.
fn pieces(arg: &str) -> Vec<Piece<'_>> {
    let bytes = arg.as_bytes();
    let (mut out, last) = (0..bytes.len()).fold((Vec::new(), 0usize), |(mut out, last), i| {
        if i < last || bytes[i] != b'{' {
            return (out, last);
        }
        match bytes[i + 1..].iter().position(|b| *b == b'{' || *b == b'}') {
            Some(off) if bytes[i + 1 + off] == b'}' => {
                out.push(Piece::Text(&arg[last..i]));
                out.push(Piece::Placeholder(&arg[i + 1..i + 1 + off]));
                (out, i + off + 2)
            }
            _ => (out, last),
        }
    });
    out.push(Piece::Text(&arg[last..]));
    out
}

/// Reject a template that uses an unknown placeholder.
///
/// # Errors
/// [`UnknownPlaceholder`] for the first unknown name.
pub fn validate_template(args: &[String]) -> Result<(), UnknownPlaceholder> {
    let unknown = args.iter().flat_map(|a| pieces(a)).find_map(|p| match p {
        Piece::Placeholder(name) if !PLACEHOLDERS.contains(&name) => Some(name.to_owned()),
        _ => None,
    });
    unknown.map_or(Ok(()), |name| Err(UnknownPlaceholder { name }))
}

/// Substitute placeholders; each template element stays exactly one argument. A name missing from
/// `values` is left as written.
pub fn render_args(args: &[String], values: &[(&str, String)]) -> Vec<String> {
    args.iter()
        .map(|arg| {
            pieces(arg)
                .into_iter()
                .map(|piece| match piece {
                    Piece::Text(t) => t.to_owned(),
                    Piece::Placeholder(name) => values
                        .iter()
                        .find(|(k, _)| *k == name)
                        .map_or_else(|| format!("{{{name}}}"), |(_, v)| v.clone()),
                })
                .collect()
        })
        .collect()
}

/// The placeholder values for a request, given the prompt file path.
pub fn placeholder_values(
    request: &AgentRunRequest,
    prompt_file: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("attempt_id", request.attempt_id.clone()),
        ("task_id", request.task_id.clone()),
        ("task_key", request.task_key.clone()),
        ("workdir", request.workdir.display().to_string()),
        ("manifest", request.manifest_path.display().to_string()),
        ("prompt_file", prompt_file.to_owned()),
    ]
}

/// The JSON request sent on stdin and written to `workgraph-request.json`.
pub fn request_payload(request: &AgentRunRequest) -> Value {
    json!({
        "protocol": PROTOCOL,
        "attempt_id": request.attempt_id,
        "task": {"id": request.task_id, "key": request.task_key},
        "prompt": request.prompt,
        "manifest": request.manifest,
    })
}

/// What a command agent can do, from its configuration.
pub fn command_capabilities(config: &CommandConfig) -> AgentCapabilities {
    AgentCapabilities {
        streaming_progress: config.supports_progress,
        usage_reporting: config.supports_usage,
        session_resume: config.supports_resume,
        isolated_workspace: true,
        cancellation: true,
        ..AgentCapabilities::default()
    }
}

/// The non-secret configuration recorded on each attempt.
pub fn command_config_snapshot(config: &CommandConfig) -> Value {
    let keys: BTreeSet<&str> = config.env.iter().map(|(k, _)| k.as_str()).collect();
    json!({
        "adapter": "command",
        "executable": config.executable,
        "args": config.args,
        "env_keys": keys,
        "timeout_seconds": config.timeout_seconds,
    })
}

/// Incrementally interprets the agent's output lines.
#[derive(Debug, Clone, Default)]
pub struct CommandState {
    pub result: Option<Value>,
    pub usage: Map<String, Value>,
    pub artifacts: Vec<Value>,
    pub progress: u64,
    pub logs: u64,
}

impl CommandState {
    fn log(&mut self, text: &str) -> Vec<AgentEvent> {
        self.logs += 1;
        vec![AgentEvent::plain(EventKind::Log, prefix(text, 2000))]
    }

    /// Interpret one output line, returning the new state and the events.
    pub fn handle_line(self, line: &str) -> (Self, Vec<AgentEvent>) {
        let mut next = self;
        let events = next.apply_line(line);
        (next, events)
    }

    fn apply_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let text = line.trim();
        if !text.starts_with('{') {
            return self.log(text);
        }
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return self.log(text);
        };
        match get(&message, "type").as_str() {
            Some("progress") => {
                self.progress += 1;
                vec![AgentEvent::with_data(
                    EventKind::Progress,
                    prefix(&py_str(get(&message, "message")), 5000),
                    json!({"percent": get(&message, "percent")}),
                )]
            }
            Some("usage") => {
                if let Value::Object(fields) = &message {
                    fields
                        .iter()
                        .filter(|(k, _)| k.as_str() != "type")
                        .filter_map(|(k, v)| as_int(v).filter(|n| *n >= 0).map(|n| (k, n)))
                        .for_each(|(k, n)| {
                            let total = self.usage.get(k).and_then(as_int).unwrap_or(0) + n;
                            self.usage.insert(k.clone(), json!(total));
                        });
                }
                vec![AgentEvent::with_data(
                    EventKind::Usage,
                    "",
                    Value::Object(self.usage.clone()),
                )]
            }
            Some("artifact") => {
                let entry: Map<String, Value> = ["name", "path", "kind"]
                    .iter()
                    .map(|k| ((*k).to_owned(), get(&message, k).clone()))
                    .collect();
                self.artifacts.push(Value::Object(entry));
                vec![AgentEvent::with_data(
                    EventKind::Artifact,
                    py_str(get(&message, "name")),
                    message,
                )]
            }
            Some("result") => {
                let summary = prefix(&py_str(get(&message, "summary")), 5000);
                self.result = Some(message.clone());
                vec![AgentEvent::with_data(EventKind::Result, summary, message)]
            }
            _ => self.log(text),
        }
    }
}

/// The attempt's result from the finished output and process.
pub fn finish_command(
    state: &CommandState,
    process: &ProcessResult,
    config: &CommandConfig,
) -> AgentRunResult {
    let null = Value::Null;
    let result = state.result.as_ref().unwrap_or(&null);
    let mut run = AgentRunResult::new(RunStatus::Failed, process.exit_code);
    run.usage.clone_from(&state.usage);
    run.telemetry = match json!({
        "progress_events": state.progress,
        "log_lines": state.logs,
        "output_lines": process.lines,
    }) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if process.timed_out {
        run.status = RunStatus::TimedOut;
        run.error = Some("agent timed out".to_owned());
        return run;
    }
    if process.cancelled {
        run.status = RunStatus::Cancelled;
        run.error = Some("cancelled".to_owned());
        return run;
    }
    let declared = get(result, "status");
    let ok = process.exit_code == Some(0)
        && (declared.is_null() || declared.as_str() == Some("succeeded"));
    let summary = result.get("summary").map(py_str).unwrap_or_default();
    run.summary = prefix(&summary, 50_000);
    run.cost_usd = as_number(get(result, "cost_usd")).filter(|_| config.supports_usage);
    run.artifacts.clone_from(&state.artifacts);
    if ok {
        run.status = RunStatus::Succeeded;
    } else {
        let reported = if truthy(get(result, "error")) {
            py_str(get(result, "error"))
        } else {
            String::new()
        };
        let error = if reported.is_empty() {
            let start = process.tail.len().saturating_sub(20);
            process.tail[start..].join("\n")
        } else {
            reported
        };
        run.error = Some(prefix(&error, 20_000));
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn template_validation_and_rendering() {
        assert!(
            validate_template(&strings(&[
                "run",
                "--m={manifest}",
                "{task_key}-{attempt_id}"
            ]))
            .is_ok()
        );
        let bad = validate_template(&strings(&["a", "{nope}"]));
        assert_eq!(bad.map_err(|e| e.name), Err("nope".to_owned()));
        // Braces that do not form `{name}` are literal text.
        assert!(validate_template(&strings(&["{", "}", "{{task_id}", "a{b"])).is_ok());
        let values = [("task_id", "T".to_owned()), ("task_key", "WG-1".to_owned())];
        assert_eq!(
            render_args(
                &strings(&["{{task_id}", "x {task_key}/{task_key}", "{"]),
                &values
            ),
            ["{T", "x WG-1/WG-1", "{"]
        );
    }

    #[test]
    fn lines_become_events() {
        let mut state = CommandState::default();
        let lines = [
            "plain text",
            r#"{"type":"progress","message":"half","percent":50}"#,
            r#"{"type":"usage","input_tokens":10,"output_tokens":2,"x":-1,"y":1.5}"#,
            r#"{"type":"usage","input_tokens":5}"#,
            r#"{"type":"artifact","name":"a","path":"p"}"#,
            r#"{"type":"mystery"}"#,
            "{broken",
            r#"{"type":"result","status":"succeeded","summary":"fine","cost_usd":0.25}"#,
        ];
        let mut events = Vec::new();
        for line in lines {
            let (next, more) = state.handle_line(line);
            state = next;
            events.extend(more);
        }
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "log", "progress", "usage", "usage", "artifact", "log", "log", "result"
            ]
        );
        assert_eq!(events[1].data.get("percent"), Some(&json!(50)));
        assert_eq!(state.usage.get("input_tokens"), Some(&json!(15)));
        assert_eq!(state.usage.len(), 2);
        assert_eq!(
            state.artifacts,
            [json!({"name": "a", "path": "p", "kind": null})]
        );
        assert_eq!((state.progress, state.logs), (1, 3));
    }

    #[test]
    fn finish_decides_status() {
        let process = |code| ProcessResult {
            argv: Vec::new(),
            exit_code: code,
            duration_ms: 1,
            timed_out: false,
            cancelled: false,
            tail: vec!["boom".into()],
            lines: 4,
            truncated_lines: 0,
            output_incomplete: false,
        };
        let mut config = CommandConfig::new("agent");
        let mut state = CommandState::default();
        assert_eq!(
            finish_command(&state, &process(Some(0)), &config).status,
            RunStatus::Succeeded
        );
        let failed = finish_command(&state, &process(Some(2)), &config);
        assert_eq!(
            (failed.status, failed.error.as_deref()),
            (RunStatus::Failed, Some("boom"))
        );
        state = state
            .handle_line(
                r#"{"type":"result","status":"failed","summary":"s","error":"why","cost_usd":1}"#,
            )
            .0;
        let declared = finish_command(&state, &process(Some(0)), &config);
        assert_eq!(
            (declared.status, declared.error.as_deref()),
            (RunStatus::Failed, Some("why"))
        );
        assert_eq!(declared.cost_usd, None);
        config.supports_usage = true;
        assert_eq!(
            finish_command(&state, &process(Some(0)), &config).cost_usd,
            Some(1.0)
        );
    }
}
