//! Claude Code hook payload normalisation (the pure steps of `integrations/claude_code/ingest.py`).
//!
//! [`normalise`] runs the steps `process_record` takes before its database work, in the same order: the
//! `Invalid` wrap of a payload that is not a hook object, the `dedupe_key`, pydantic-style validation of
//! the common fields, the redacted copy of the payload, the `_workgraph` stamp and the received time.
//! The shell then does the database work with the result.

use regex::Regex;
use serde_json::{Map, Value, json};
use wg_hash::sha256_hex;
use wg_types::Timestamp;

use crate::json::{dumps_sorted, py_str, truthy};
use crate::redact::{redact, redact_object, redact_text, summarize};

/// The event name of a payload that is not a hook object.
pub const INVALID_EVENT: &str = "Invalid";
/// The longest hook event name the hook event row keeps.
pub const EVENT_NAME_CHARS: usize = 64;
/// The longest tool name a tool event row keeps.
pub const TOOL_NAME_CHARS: usize = 200;
/// The longest file path a tool summary keeps.
pub const FILE_PATH_CHARS: usize = 500;
/// The longest Bash command text a tool summary keeps before its head word is taken.
pub const COMMAND_CHARS: usize = 300;
/// The bound on an error excerpt stored on a tool event.
pub const ERROR_EXCERPT_CHARS: usize = 1000;
/// The bound on a stored subagent or internal-task text.
pub const TEXT_CHARS: usize = 2000;
/// The bound on a stored internal-task subject.
pub const SUBJECT_CHARS: usize = 500;
/// The floor of the bound applied to a whole sanitised payload.
pub const MIN_PAYLOAD_CHARS: usize = 2000;
/// The schema version of a payload that carries neither `prompt_id` nor `effort`.
pub const SCHEMA_V1: u8 = 1;
/// The schema version of a payload that carries `prompt_id` or `effort`.
pub const SCHEMA_V2: u8 = 2;

/// The `Workgraph Task-ID` pattern used to find task keys in free text.
const TASK_KEY: &str = r"\b([A-Z][A-Z0-9]{0,9}-\d+)\b";

/// The two settings that bound what a hook keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Keep redacted tool payloads (true) or only their shape (false).
    pub capture_tool_payloads: bool,
    /// The bound on a kept payload string, in code points.
    pub max_payload_chars: usize,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            capture_tool_payloads: false,
            max_payload_chars: 4000,
        }
    }
}

/// A payload field that does not validate. The message names the field and the expected type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadError {
    pub field: String,
    pub message: String,
}

/// The validated common fields of a hook payload. Unknown fields are tolerated and not kept here.
#[derive(Clone, Debug, PartialEq)]
pub struct HookFields {
    pub hook_event_name: String,
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub permission_mode: Option<String>,
    pub prompt_id: Option<String>,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub source: Option<String>,
    pub reason: Option<String>,
    pub model: Value,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub tool_input: Value,
    pub tool_response: Value,
    pub error: Option<String>,
    pub is_interrupt: Option<bool>,
    pub duration_ms: Option<i64>,
    pub agent_transcript_path: Option<String>,
    pub last_assistant_message: Option<String>,
    pub stop_hook_active: Option<bool>,
    pub task_id: Option<String>,
    pub task_subject: Option<String>,
    pub task_description: Option<String>,
    pub teammate_name: Option<String>,
}

/// The result of normalising one spooled or direct hook record.
#[derive(Clone, Debug, PartialEq)]
pub struct Normalised {
    /// The event name, `Invalid` for a payload that is not a hook object.
    pub event: String,
    /// The stable id of the received record.
    pub dedupe_key: String,
    /// The redacted payload, with the `_workgraph` stamp last.
    pub sanitized: Map<String, Value>,
    pub schema_version: u8,
    pub fields: HookFields,
    /// The received time; `None` when the record has no `received_at` text, and the shell uses its clock.
    pub received_at: Option<Timestamp>,
    /// The tool summary of the payload (see [`tool_summary`]).
    pub tool_summary: Map<String, Value>,
}

/// The Python `dedupe_key`: SHA-256 of `json.dumps(payload, sort_keys=True)`, `|` and `str(received_at)`.
pub fn dedupe_key(record: &Value) -> String {
    let payload = dumps_sorted(record.get("payload").unwrap_or(&Value::Null));
    let received = py_str(record.get("received_at").unwrap_or(&Value::Null));
    sha256_hex(format!("{payload}|{received}").as_bytes())
}

/// Schema v2 for payloads with `prompt_id` or `effort`, else v1.
pub fn schema_version(raw: &Map<String, Value>) -> u8 {
    if raw.contains_key("prompt_id") || raw.contains_key("effort") {
        SCHEMA_V2
    } else {
        SCHEMA_V1
    }
}

/// Runs the normalisation steps of `process_record` on one record. `source` is `direct` or `spool`.
pub fn normalise(
    record: &Value,
    config: CaptureConfig,
    source: &str,
) -> Result<Normalised, PayloadError> {
    let payload = record
        .get("payload")
        .filter(|value| truthy(Some(value)))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let raw = hook_object(payload);
    let fields = validate(&raw)?;
    let key = dedupe_key(record);
    let version = schema_version(&raw);
    let sanitized = sanitize(&raw, config)
        .into_iter()
        .chain([(
            "_workgraph".to_owned(),
            json!({"schema_version": version, "source": source}),
        )])
        .collect();
    let received_at = match record.get("received_at") {
        Some(Value::String(text)) => Some(parse_received(text)?),
        _ => None,
    };
    Ok(Normalised {
        event: fields.hook_event_name.clone(),
        dedupe_key: key,
        sanitized,
        schema_version: version,
        tool_summary: tool_summary(&fields),
        fields,
        received_at,
    })
}

/// The payload as a hook object: a dict with `hook_event_name`, else the `Invalid` wrap that keeps the
/// original value under `original`.
fn hook_object(payload: Value) -> Map<String, Value> {
    match payload {
        Value::Object(map) if map.contains_key("hook_event_name") => map,
        original => [
            ("hook_event_name".to_owned(), json!(INVALID_EVENT)),
            ("original".to_owned(), original),
        ]
        .into_iter()
        .collect(),
    }
}

/// A received timestamp. A naive value is taken as UTC, as `ensure_utc` does.
fn parse_received(text: &str) -> Result<Timestamp, PayloadError> {
    let has_offset =
        text.ends_with('Z') || text.get(10..).is_some_and(|rest| rest.contains(['+', '-']));
    let candidate = if text.ends_with('Z') {
        text.trim_end_matches('Z').to_owned() + "+00:00"
    } else if has_offset {
        text.to_owned()
    } else {
        text.to_owned() + "+00:00"
    };
    Timestamp::parse_iso(&candidate).map_err(|_| PayloadError {
        field: "received_at".to_owned(),
        message: format!("invalid timestamp {text:?}"),
    })
}

/// Pydantic-style validation of the common hook fields. A string field takes a string or null; `hook_event_name`
/// must be a string; the bool and integer fields take their Python lax forms.
pub fn validate(raw: &Map<String, Value>) -> Result<HookFields, PayloadError> {
    Ok(HookFields {
        hook_event_name: required_str(raw, "hook_event_name")?,
        session_id: opt_str(raw, "session_id")?,
        transcript_path: opt_str(raw, "transcript_path")?,
        cwd: opt_str(raw, "cwd")?,
        permission_mode: opt_str(raw, "permission_mode")?,
        prompt_id: opt_str(raw, "prompt_id")?,
        agent_id: opt_str(raw, "agent_id")?,
        agent_type: opt_str(raw, "agent_type")?,
        source: opt_str(raw, "source")?,
        reason: opt_str(raw, "reason")?,
        model: raw.get("model").cloned().unwrap_or(Value::Null),
        tool_name: opt_str(raw, "tool_name")?,
        tool_use_id: opt_str(raw, "tool_use_id")?,
        tool_input: raw.get("tool_input").cloned().unwrap_or(Value::Null),
        tool_response: raw.get("tool_response").cloned().unwrap_or(Value::Null),
        error: opt_str(raw, "error")?,
        is_interrupt: opt_bool(raw, "is_interrupt")?,
        duration_ms: opt_int(raw, "duration_ms")?,
        agent_transcript_path: opt_str(raw, "agent_transcript_path")?,
        last_assistant_message: opt_str(raw, "last_assistant_message")?,
        stop_hook_active: opt_bool(raw, "stop_hook_active")?,
        task_id: opt_str(raw, "task_id")?,
        task_subject: opt_str(raw, "task_subject")?,
        task_description: opt_str(raw, "task_description")?,
        teammate_name: opt_str(raw, "teammate_name")?,
    })
}

fn error(field: &str, expected: &str) -> PayloadError {
    PayloadError {
        field: field.to_owned(),
        message: format!("input should be {expected}"),
    }
}

fn required_str(raw: &Map<String, Value>, key: &str) -> Result<String, PayloadError> {
    match raw.get(key) {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(error(key, "a valid string")),
    }
}

fn opt_str(raw: &Map<String, Value>, key: &str) -> Result<Option<String>, PayloadError> {
    match raw.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(error(key, "a valid string")),
    }
}

fn opt_bool(raw: &Map<String, Value>, key: &str) -> Result<Option<bool>, PayloadError> {
    match raw.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(Value::Number(number)) if number.as_i64() == Some(0) => Ok(Some(false)),
        Some(Value::Number(number)) if number.as_i64() == Some(1) => Ok(Some(true)),
        Some(_) => Err(error(key, "a valid boolean")),
    }
}

fn opt_int(raw: &Map<String, Value>, key: &str) -> Result<Option<i64>, PayloadError> {
    match raw.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|x| x.fract() == 0.0)
                    .map(|x| x as i64)
            })
            .map(Some)
            .ok_or_else(|| error(key, "a valid integer")),
        Some(_) => Err(error(key, "a valid integer")),
    }
}

/// The redacted, bounded copy of a payload (`_sanitize`). The caller appends the `_workgraph` stamp.
pub fn sanitize(payload: &Map<String, Value>, config: CaptureConfig) -> Map<String, Value> {
    let bound = config.max_payload_chars;
    let entries: Map<String, Value> = payload
        .iter()
        .map(|(key, value)| (key.clone(), sanitize_entry(key, value, config)))
        .collect();
    redact_object(&entries, bound.max(MIN_PAYLOAD_CHARS))
}

fn sanitize_entry(key: &str, value: &Value, config: CaptureConfig) -> Value {
    let bound = config.max_payload_chars;
    match key {
        "tool_input" | "tool_response" => {
            if config.capture_tool_payloads {
                redact(value, bound)
            } else {
                json!({"summary": summarize(value)})
            }
        }
        "prompt" => {
            let text = py_str(value);
            if config.capture_tool_payloads {
                Value::String(redact_text(&text, Some(bound)))
            } else {
                json!({"length": text.chars().count()})
            }
        }
        "last_assistant_message" | "task_description" | "error" => match value.as_str() {
            Some(text) => Value::String(redact_text(text, Some(TEXT_CHARS))),
            None => value.clone(),
        },
        _ => value.clone(),
    }
}

/// The searchable summary of a tool call: the file a file tool touched, the head word of a Bash command, and
/// the server of an MCP tool. Keys are absent when the payload does not carry them.
pub fn tool_summary(fields: &HookFields) -> Map<String, Value> {
    let input = fields.tool_input.as_object();
    let name = fields.tool_name.as_deref();
    let file = match (name, input.and_then(|map| map.get("file_path"))) {
        (Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Read"), Some(path))
            if truthy(Some(path)) =>
        {
            Some((
                "file_path".to_owned(),
                json!(
                    py_str(path)
                        .chars()
                        .take(FILE_PATH_CHARS)
                        .collect::<String>()
                ),
            ))
        }
        _ => None,
    };
    let bash = match (name, input.and_then(|map| map.get("command"))) {
        (Some("Bash"), Some(command)) if truthy(Some(command)) => {
            let text = redact_text(&py_str(command), Some(COMMAND_CHARS));
            let head = text.split_whitespace().next().unwrap_or("").to_owned();
            Some(("command_head".to_owned(), json!(head)))
        }
        _ => None,
    };
    let mcp = name.filter(|tool| tool.starts_with("mcp__")).map(|tool| {
        let server = tool
            .split("__")
            .nth(1)
            .map_or(Value::Null, |part| json!(part));
        ("mcp_server".to_owned(), server)
    });
    [file, bash, mcp].into_iter().flatten().collect()
}

/// The phase a tool hook event records, or `None` for any other event.
pub fn tool_phase(event: &str) -> Option<&'static str> {
    match event {
        "PreToolUse" => Some("pre"),
        "PostToolUse" => Some("success"),
        "PostToolUseFailure" => Some("failure"),
        _ => None,
    }
}

/// A tool event row, as the store keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolEventRow {
    pub phase: &'static str,
    pub tool_name: String,
    pub tool_use_id: Option<String>,
    pub duration_ms: Option<i64>,
    pub error_excerpt: Option<String>,
    /// The tool summary plus `is_interrupt`.
    pub summary: Map<String, Value>,
}

/// The tool event a hook payload records, if its event is a tool phase and it names a tool.
pub fn tool_event(fields: &HookFields, summary: &Map<String, Value>) -> Option<ToolEventRow> {
    let phase = tool_phase(&fields.hook_event_name)?;
    let name = fields
        .tool_name
        .as_deref()
        .filter(|name| !name.is_empty())?;
    Some(ToolEventRow {
        phase,
        tool_name: name.chars().take(TOOL_NAME_CHARS).collect(),
        tool_use_id: fields.tool_use_id.clone(),
        duration_ms: fields.duration_ms,
        error_excerpt: fields
            .error
            .as_deref()
            .filter(|error| !error.is_empty())
            .map(|error| redact_text(error, Some(ERROR_EXCERPT_CHARS))),
        summary: summary
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .chain([(
                "is_interrupt".to_owned(),
                fields.is_interrupt.map_or(Value::Null, Value::Bool),
            )])
            .collect(),
    })
}

/// How a tool event moves the session counters: `(tool calls, tool failures)`. Only the completed phases count.
pub fn tool_counters(phase: &str) -> (i64, i64) {
    match phase {
        "pre" => (0, 0),
        "failure" => (1, 1),
        _ => (1, 0),
    }
}

/// The stored excerpt of a subagent's last message: redacted and bounded, or `None` when empty.
pub fn last_message_excerpt(last_message: Option<&str>) -> Option<String> {
    Some(redact_text(
        last_message.unwrap_or(""),
        Some(ERROR_EXCERPT_CHARS),
    ))
    .filter(|text| !text.is_empty())
}

/// The stored subject of an internal task: its first 500 code points.
pub fn internal_task_subject(subject: Option<&str>) -> String {
    subject.unwrap_or("").chars().take(SUBJECT_CHARS).collect()
}

/// The stored description of an internal task: redacted and bounded, or `None` when empty.
pub fn internal_task_description(description: Option<&str>) -> Option<String> {
    Some(redact_text(description.unwrap_or(""), Some(TEXT_CHARS))).filter(|text| !text.is_empty())
}

/// The telemetry object of an attempt after a session rolls up its hook counters. Existing keys keep their
/// position, and the three hook counters are set.
pub fn rollup_telemetry(
    existing: &Map<String, Value>,
    tool_calls: i64,
    tool_failures: i64,
    subagents: usize,
) -> Map<String, Value> {
    existing
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .chain([
            ("hook_tool_calls".to_owned(), json!(tool_calls)),
            ("hook_tool_failures".to_owned(), json!(tool_failures)),
            ("hook_subagents".to_owned(), json!(subagents)),
        ])
        .collect()
}

/// The `Task-ID` keys named in free text, in order, duplicates included.
pub fn task_keys_in(text: &str) -> Vec<String> {
    Regex::new(TASK_KEY).map_or_else(
        |_| Vec::new(),
        |pattern| {
            pattern
                .captures_iter(text)
                .filter_map(|captures| captures.get(1))
                .map(|found| found.as_str().to_owned())
                .collect()
        },
    )
}
