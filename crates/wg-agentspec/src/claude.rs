//! The Claude Code execution adapter (P02), pure part.
//!
//! Uses the documented programmatic interface: `claude -p --output-format stream-json --verbose`.
//! Each stdout line is a JSON message (`system`/`assistant`/`user`/`result`); human-oriented
//! terminal output is never scraped. The session id is fixed up front with `--session-id` so hooks
//! fired by that session correlate with the attempt automatically.

use serde_json::{Map, Value, json};
use wg_procspec::{ProcError, ProcessResult};

use crate::types::{
    AgentCapabilities, AgentEvent, AgentRunRequest, AgentRunResult, EventKind, RunStatus,
};
use crate::util::{as_int, as_number, get, py_float, py_str, truncate_chars, truthy};

const USAGE_FIELDS: [&str; 4] = [
    "input_tokens",
    "output_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
];

/// `ClaudeCodeAgentConfig`: how to launch Claude Code.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeConfig {
    pub executable: String,
    pub model: Option<String>,
    pub permission_mode: String,
    pub allowed_tools: Vec<String>,
    pub max_turns: Option<u64>,
    pub max_budget_usd: Option<f64>,
    pub timeout_seconds: f64,
    pub extra_args: Vec<String>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            executable: "claude".to_owned(),
            model: None,
            permission_mode: "acceptEdits".to_owned(),
            allowed_tools: Vec::new(),
            max_turns: None,
            max_budget_usd: None,
            timeout_seconds: 3600.0,
            extra_args: Vec::new(),
        }
    }
}

/// The Claude Code provider: a name and its configuration. Running it is `wg-agents`.
#[derive(Debug, Clone)]
pub struct ClaudeCodeProvider {
    pub name: String,
    pub config: ClaudeConfig,
}

impl ClaudeCodeProvider {
    /// The adapter type name.
    pub const ADAPTER_TYPE: &'static str = "claude_code";

    /// A provider named `name`.
    pub fn new(name: impl Into<String>, config: ClaudeConfig) -> Self {
        Self {
            name: name.into(),
            config,
        }
    }

    /// What Claude Code can do.
    pub fn capabilities(&self) -> AgentCapabilities {
        claude_capabilities()
    }

    /// The non-secret configuration recorded on each attempt.
    pub fn config_snapshot(&self) -> Value {
        claude_config_snapshot(&self.config)
    }
}

/// What Claude Code can do.
pub fn claude_capabilities() -> AgentCapabilities {
    AgentCapabilities {
        streaming_progress: true,
        usage_reporting: true,
        cost_reporting: true,
        session_resume: true,
        isolated_workspace: true,
        cancellation: true,
        subagents: true,
        lifecycle_hooks: true,
    }
}

/// The non-secret configuration recorded on each attempt.
pub fn claude_config_snapshot(config: &ClaudeConfig) -> Value {
    json!({
        "adapter": "claude_code",
        "executable": config.executable,
        "model": config.model,
        "permission_mode": config.permission_mode,
        "allowed_tools": config.allowed_tools,
        "max_turns": config.max_turns,
        "max_budget_usd": config.max_budget_usd,
        "extra_args": config.extra_args,
    })
}

/// The argument vector (the executable first). The prompt is not in it: it goes on stdin.
pub fn build_argv(
    config: &ClaudeConfig,
    request: &AgentRunRequest,
    session_id: &str,
) -> Vec<String> {
    let session = match &request.resume_session_id {
        Some(resume) if !resume.is_empty() => vec!["--resume".to_owned(), resume.clone()],
        _ => vec!["--session-id".to_owned(), session_id.to_owned()],
    };
    let model = request
        .model
        .as_ref()
        .filter(|m| !m.is_empty())
        .or(config.model.as_ref().filter(|m| !m.is_empty()))
        .map(|m| vec!["--model".to_owned(), m.clone()])
        .unwrap_or_default();
    let tools = if config.allowed_tools.is_empty() {
        Vec::new()
    } else {
        std::iter::once("--allowedTools".to_owned())
            .chain(config.allowed_tools.iter().cloned())
            .collect()
    };
    let turns = config
        .max_turns
        .filter(|n| *n != 0)
        .map(|n| vec!["--max-turns".to_owned(), n.to_string()])
        .unwrap_or_default();
    let budget = config
        .max_budget_usd
        .filter(|b| *b != 0.0)
        .map(|b| vec!["--max-budget-usd".to_owned(), py_float(b)])
        .unwrap_or_default();
    let dirs = request
        .additional_dirs
        .iter()
        .flat_map(|d| ["--add-dir".to_owned(), d.display().to_string()]);
    std::iter::once(config.executable.clone())
        .chain(
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
            ]
            .map(str::to_owned),
        )
        .chain(std::iter::once(config.permission_mode.clone()))
        .chain(session)
        .chain(model)
        .chain(tools)
        .chain(turns)
        .chain(budget)
        .chain(dirs)
        .chain(config.extra_args.iter().cloned())
        .collect()
}

/// The effective timeout in milliseconds: the smaller of the request's and the configured one.
pub fn timeout_ms(request_seconds: f64, config_seconds: f64) -> u64 {
    let ms = request_seconds.min(config_seconds) * 1000.0;
    if ms.is_finite() && ms > 0.0 {
        // Saturating float-to-int conversion; the value is positive and finite.
        ms as u64
    } else {
        0
    }
}

/// The availability verdict from the `--version` probe: success is exit code 0, the detail is the
/// first 200 characters of stdout, else of stderr.
pub fn availability_detail(exit_code: Option<i32>, stdout: &str, stderr: &str) -> (bool, String) {
    let text = if stdout.is_empty() { stderr } else { stdout };
    (exit_code == Some(0), truncate_chars(text.trim(), 200))
}

/// The unavailability message for a resolve failure, as the Python `ProviderUnavailableError`.
pub fn unavailable_message(error: &ProcError) -> String {
    match error {
        ProcError::NotFound(name) => {
            let path = std::path::Path::new(name);
            if path.is_absolute() || path.components().count() > 1 {
                format!("executable {name} does not exist")
            } else {
                format!("executable '{name}' was not found on PATH")
            }
        }
        other => other.to_string(),
    }
}

/// Incrementally interprets Claude Code `stream-json` messages.
#[derive(Debug, Clone, Default)]
pub struct StreamState {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub tool_uses: u64,
    pub tool_errors: u64,
    /// Tool name and call count, in first-use order.
    pub tools: Vec<(String, u64)>,
    pub assistant_messages: u64,
    pub last_text: String,
    pub result: Option<Value>,
    pub subagent_tool_uses: u64,
    /// Lines that were not JSON.
    pub stray: u64,
}

fn text_or(value: &Value, fallback: &Option<String>) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => fallback.clone(),
    }
}

fn content_blocks(message: &Value) -> Vec<&Value> {
    match get(get(message, "message"), "content") {
        Value::Array(items) => items.iter().collect(),
        _ => Vec::new(),
    }
}

impl StreamState {
    /// A state with the session id known up front.
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: Some(session_id.to_owned()),
            ..Self::default()
        }
    }

    /// Interpret one output line, returning the new state and the events. Blank lines are
    /// skipped; a line that is not a JSON object is kept as a `log` event (and counted as stray
    /// when it is not JSON at all).
    pub fn handle_line(self, line: &str) -> (Self, Vec<AgentEvent>) {
        let mut next = self;
        let events = next.apply_line(line);
        (next, events)
    }

    /// Interpret one decoded message, returning the new state and the events it produces.
    pub fn handle(self, message: &Value) -> (Self, Vec<AgentEvent>) {
        let mut next = self;
        let events = next.apply(message);
        (next, events)
    }

    fn apply_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let text = line.trim();
        if text.is_empty() {
            return Vec::new();
        }
        match serde_json::from_str::<Value>(text) {
            Err(_) => {
                self.stray += 1;
                vec![AgentEvent::plain(
                    EventKind::Log,
                    truncate_chars(text, 2000),
                )]
            }
            Ok(message @ Value::Object(_)) => self.apply(&message),
            Ok(_) => Vec::new(),
        }
    }

    fn apply(&mut self, message: &Value) -> Vec<AgentEvent> {
        match (
            get(message, "type").as_str(),
            get(message, "subtype").as_str(),
        ) {
            (Some("system"), Some("init")) => {
                self.session_id = text_or(get(message, "session_id"), &self.session_id);
                self.model = text_or(get(message, "model"), &self.model);
                let tools = match get(message, "tools") {
                    Value::Array(items) => items.len(),
                    _ => 0,
                };
                vec![AgentEvent::with_data(
                    EventKind::Session,
                    "session started",
                    json!({"session_id": self.session_id, "model": self.model, "tools": tools}),
                )]
            }
            (Some("assistant"), _) => {
                self.assistant_messages += 1;
                content_blocks(message)
                    .into_iter()
                    .filter_map(|block| self.assistant_block(block))
                    .collect()
            }
            (Some("user"), _) => content_blocks(message)
                .into_iter()
                .filter(|b| {
                    get(b, "type").as_str() == Some("tool_result") && truthy(get(b, "is_error"))
                })
                .map(|block| {
                    self.tool_errors += 1;
                    AgentEvent::with_data(
                        EventKind::ToolResult,
                        "tool error",
                        json!({"tool_use_id": get(block, "tool_use_id")}),
                    )
                })
                .collect(),
            (Some("result"), _) => {
                self.session_id = text_or(get(message, "session_id"), &self.session_id);
                self.result = Some(message.clone());
                vec![AgentEvent::with_data(
                    EventKind::Result,
                    truncate_chars(&py_str(get(message, "result")), 2000),
                    json!({
                        "subtype": get(message, "subtype"),
                        "is_error": get(message, "is_error"),
                    }),
                )]
            }
            _ => Vec::new(),
        }
    }

    fn assistant_block(&mut self, block: &Value) -> Option<AgentEvent> {
        match get(block, "type").as_str() {
            Some("text") if truthy(get(block, "text")) => {
                let text = py_str(get(block, "text"));
                self.last_text.clone_from(&text);
                Some(AgentEvent::plain(
                    EventKind::Message,
                    truncate_chars(&text, 2000),
                ))
            }
            Some("tool_use") => {
                self.tool_uses += 1;
                let name = match get(block, "name") {
                    Value::Null => "unknown".to_owned(),
                    other => py_str(other),
                };
                match self.tools.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, count)) => *count += 1,
                    None => self.tools.push((name.clone(), 1)),
                }
                if name == "Task" || name == "Agent" {
                    self.subagent_tool_uses += 1;
                }
                Some(AgentEvent::with_data(
                    EventKind::ToolUse,
                    name,
                    json!({"tool_use_id": get(block, "id")}),
                ))
            }
            _ => None,
        }
    }

    /// The usage the result reported: integer token counts plus `num_turns`. Empty before a result.
    pub fn usage(&self) -> Map<String, Value> {
        let Some(result) = &self.result else {
            return Map::new();
        };
        let raw = get(result, "usage");
        let mut usage: Map<String, Value> = USAGE_FIELDS
            .iter()
            .filter_map(|k| as_int(get(raw, k)).map(|n| ((*k).to_owned(), json!(n))))
            .collect();
        if let Some(turns) = as_int(get(result, "num_turns")) {
            usage.insert("num_turns".to_owned(), json!(turns));
        }
        usage
    }
}

/// The attempt's result from the finished stream and process.
pub fn finish_claude(state: &StreamState, process: &ProcessResult) -> AgentRunResult {
    let null = Value::Null;
    let result = state.result.as_ref().unwrap_or(&null);
    let tools: Map<String, Value> = state
        .tools
        .iter()
        .map(|(n, c)| (n.clone(), json!(c)))
        .collect();
    let telemetry = json!({
        "tool_uses": state.tool_uses,
        "tool_errors": state.tool_errors,
        "tools": tools,
        "assistant_messages": state.assistant_messages,
        "subagent_launches": state.subagent_tool_uses,
        "unparsed_lines": state.stray,
    });
    let mut run = AgentRunResult::new(RunStatus::Failed, process.exit_code);
    run.session_id.clone_from(&state.session_id);
    run.model.clone_from(&state.model);
    run.usage = state.usage();
    run.cost_usd = as_number(get(result, "total_cost_usd"));
    run.provider_duration_ms = as_int(get(result, "duration_ms"));
    run.num_turns = as_int(get(result, "num_turns"));
    run.telemetry = match telemetry {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if process.timed_out {
        run.status = RunStatus::TimedOut;
        run.error = Some("Claude Code timed out".to_owned());
        return run;
    }
    if process.cancelled {
        run.status = RunStatus::Cancelled;
        run.error = Some("cancelled".to_owned());
        return run;
    }
    let success = process.exit_code == Some(0)
        && get(result, "subtype").as_str() == Some("success")
        && !truthy(get(result, "is_error"));
    let reported = [get(result, "result"), get(result, "subtype")]
        .into_iter()
        .find(|v| truthy(v))
        .map(py_str)
        .unwrap_or_default();
    let error = if reported.is_empty() {
        let start = process.tail.len().saturating_sub(20);
        process.tail[start..].join("\n")
    } else {
        reported
    };
    let summary = if truthy(get(result, "result")) {
        py_str(get(result, "result"))
    } else {
        state.last_text.clone()
    };
    run.summary = truncate_chars(&summary, 50_000);
    if success {
        run.status = RunStatus::Succeeded;
    } else if !error.is_empty() {
        run.error = Some(truncate_chars(&error, 20_000));
    }
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn request() -> AgentRunRequest {
        AgentRunRequest {
            attempt_id: "a".into(),
            task_id: "t".into(),
            task_key: "WG-1".into(),
            prompt: "p".into(),
            workdir: PathBuf::from("w"),
            manifest_path: PathBuf::from("m.json"),
            manifest: json!({}),
            timeout_seconds: 10.0,
            log_path: PathBuf::from("l.log"),
            env: Vec::new(),
            model: None,
            resume_session_id: None,
            additional_dirs: vec![PathBuf::from("extra")],
        }
    }

    #[test]
    fn argv_new_and_resumed_session() {
        let config = ClaudeConfig {
            model: Some("m1".into()),
            allowed_tools: vec!["Edit".into(), "Bash".into()],
            max_turns: Some(7),
            max_budget_usd: Some(5.0),
            extra_args: vec!["--x".into()],
            ..ClaudeConfig::default()
        };
        let argv = build_argv(&config, &request(), "sid");
        assert_eq!(
            argv.join(" "),
            "claude -p --output-format stream-json --verbose --permission-mode acceptEdits \
             --session-id sid --model m1 --allowedTools Edit Bash --max-turns 7 \
             --max-budget-usd 5.0 --add-dir extra --x"
        );
        let mut resumed = request();
        resumed.resume_session_id = Some("old".into());
        resumed.model = Some("m2".into());
        let argv = build_argv(&ClaudeConfig::default(), &resumed, "sid");
        assert!(argv.join(" ").contains("--resume old --model m2"));
        assert!(!argv.contains(&"--session-id".to_owned()));
    }

    #[test]
    fn timeout_is_the_smaller() {
        assert_eq!(timeout_ms(10.0, 3600.0), 10_000);
        assert_eq!(timeout_ms(3600.0, 0.5), 500);
    }

    #[test]
    fn availability() {
        assert_eq!(
            availability_detail(Some(0), " 2.1 (Claude Code)\n", "x"),
            (true, "2.1 (Claude Code)".to_owned())
        );
        assert_eq!(
            availability_detail(Some(1), "", "boom"),
            (false, "boom".to_owned())
        );
        assert_eq!(
            unavailable_message(&ProcError::NotFound("claude".into())),
            "executable 'claude' was not found on PATH"
        );
    }

    #[test]
    fn stream_counts_and_events() {
        let mut state = StreamState::new("s");
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s2","model":"m","tools":["a","b"]}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","id":"t1","name":"Edit"},{"type":"tool_use","id":"t2","name":"Task"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true}]}}"#,
            "not json",
            "",
            r#"{"type":"result","subtype":"success","result":"done","usage":{"input_tokens":9,"output_tokens":1.5},"num_turns":3}"#,
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
                "session",
                "message",
                "tool_use",
                "tool_use",
                "tool_result",
                "log",
                "result"
            ]
        );
        assert_eq!(state.session_id.as_deref(), Some("s2"));
        assert_eq!(
            (state.tool_uses, state.tool_errors, state.subagent_tool_uses),
            (2, 1, 1)
        );
        assert_eq!(state.stray, 1);
        assert_eq!(state.usage().get("input_tokens"), Some(&json!(9)));
        assert_eq!(state.usage().get("output_tokens"), None);
        assert_eq!(state.usage().get("num_turns"), Some(&json!(3)));
    }

    #[test]
    fn finish_statuses() {
        let mut state = StreamState::new("s");
        state.last_text = "last".into();
        let process = |code, timed_out, cancelled| ProcessResult {
            argv: Vec::new(),
            exit_code: code,
            duration_ms: 1,
            timed_out,
            cancelled,
            tail: vec!["tail line".into()],
            lines: 1,
            truncated_lines: 0,
            output_incomplete: false,
        };
        let failed = finish_claude(&state, &process(Some(1), false, false));
        assert_eq!(failed.status, RunStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("tail line"));
        assert_eq!(failed.summary, "last");
        assert_eq!(
            finish_claude(&state, &process(None, true, false)).status,
            RunStatus::TimedOut
        );
        assert_eq!(
            finish_claude(&state, &process(None, false, true)).status,
            RunStatus::Cancelled
        );
        state = state.handle_line(r#"{"type":"result","subtype":"success","result":"ok","total_cost_usd":0.5,"duration_ms":7}"#).0;
        let done = finish_claude(&state, &process(Some(0), false, false));
        assert_eq!(done.status, RunStatus::Succeeded);
        assert_eq!(
            (
                done.summary.as_str(),
                done.cost_usd,
                done.provider_duration_ms
            ),
            ("ok", Some(0.5), Some(7))
        );
        state = state.handle_line(r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"bad"}"#).0;
        let bad = finish_claude(&state, &process(Some(0), false, false));
        assert_eq!(
            (bad.status, bad.error.as_deref()),
            (RunStatus::Failed, Some("bad"))
        );
    }
}
