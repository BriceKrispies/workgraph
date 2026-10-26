//! Measured telemetry from a Claude Code subagent transcript (`integrations/claude_code/transcript.py`).
//!
//! The functions take the transcript's lines (and the sidecar's text) rather than a path: reading the
//! file is shell work, and `None` lines stand for a missing or unreadable file.
//!
//! Dedupe rule: one API message is split over several JSONL lines that share `message.id`. Usage is taken
//! from the line with the largest `output_tokens` per message id (ties: the last line). Tool uses are
//! counted once per `tool_use` id. Synthetic messages (`<synthetic>`) are ignored entirely.

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::timing::{SYNTHETIC_MODEL, breakdown};
use crate::util::{count_in_order, group_in_order, most_common, unique_in_order};

/// The usage fields summed over the turns, in report order.
pub const USAGE_FIELDS: [&str; 4] = [
    "input_tokens",
    "output_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
];

/// Token usage summed over a transcript's turns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

/// What a subagent transcript says about its run.
#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptStats {
    /// Model name to the number of assistant messages, in first-seen order.
    pub models: Vec<(String, u64)>,
    /// The dominant model: the first of the most frequent.
    pub model: Option<String>,
    pub usage: Usage,
    /// Distinct assistant message ids.
    pub turns: usize,
    /// Tool name to calls, deduplicated per `tool_use` id, in first-seen order.
    pub tool_uses: Vec<(String, u64)>,
    /// `tool_result` blocks flagged `is_error`.
    pub tool_errors: u64,
    /// Length in code points of the first user prompt.
    pub prompt_chars: usize,
    /// `Task-ID:` markers in the first prompt.
    pub task_keys: Vec<String>,
    /// `Trial-ID:` markers in the first prompt.
    pub trial_ids: Vec<String>,
    pub claude_version: Option<String>,
    /// The first record timestamp.
    pub started_at: Option<String>,
    /// The last record timestamp.
    pub ended_at: Option<String>,
    /// Sidecar: the alias the parent asked for.
    pub requested_model: Option<String>,
    /// Sidecar description.
    pub description: Option<String>,
    /// Sidecar agent type.
    pub agent_type: Option<String>,
    /// Where the wall time went; see [`crate::timing::breakdown`].
    pub time: Value,
}

impl TranscriptStats {
    /// The JSON form stored with a run: every field, with the usage object alongside the telemetry.
    pub fn to_json(&self) -> Value {
        json!({
            "models": counts_object(&self.models),
            "model": self.model,
            "requested_model": self.requested_model,
            "turns": self.turns,
            "tool_uses": counts_object(&self.tool_uses),
            "tool_errors": self.tool_errors,
            "prompt_chars": self.prompt_chars,
            "task_keys": self.task_keys,
            "trial_ids": self.trial_ids,
            "claude_version": self.claude_version,
            "started_at": self.started_at,
            "ended_at": self.ended_at,
            "description": self.description,
            "agent_type": self.agent_type,
            "time": self.time,
            "usage": {
                "input_tokens": self.usage.input_tokens,
                "output_tokens": self.usage.output_tokens,
                "cache_creation_input_tokens": self.usage.cache_creation_input_tokens,
                "cache_read_input_tokens": self.usage.cache_read_input_tokens,
            },
        })
    }
}

fn counts_object(counts: &[(String, u64)]) -> Value {
    Value::Object(
        counts
            .iter()
            .map(|(name, count)| (name.clone(), json!(count)))
            .collect(),
    )
}

/// One tool call a transcript made.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub input: Value,
    pub tool_use_id: String,
}

/// One API message's usage, as the line with the largest output count gave it.
#[derive(Clone, Debug)]
struct Turn {
    model: Option<String>,
    output_tokens: u64,
    usage: Map<String, Value>,
}

/// Python `str.splitlines()`: the lines of `text`, without their terminators. A `\r\n` pair is one break,
/// and a final break does not start another line.
pub fn split_lines(text: &str) -> Vec<String> {
    let unified: String = text
        .replace("\r\n", "\n")
        .chars()
        .map(|c| match c {
            '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => '\n',
            other => other,
        })
        .collect();
    // A final break leaves an empty last piece, and empty text is one empty piece; neither is a line.
    let pieces: Vec<String> = unified.split('\n').map(str::to_owned).collect();
    let keep = pieces.last().is_none_or(|last| !last.is_empty());
    let count = pieces.len();
    pieces
        .into_iter()
        .take(if keep { count } else { count.saturating_sub(1) })
        .collect()
}

/// The JSON objects among `lines`, each with its line number (the number counts every line, skipped or not).
fn records(lines: &[String]) -> Vec<(usize, Value)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(number, line)| {
            serde_json::from_str::<Value>(line)
                .ok()
                .filter(Value::is_object)
                .map(|record| (number, record))
        })
        .collect()
}

fn object_field<'a>(record: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    record.get(key).and_then(Value::as_object)
}

fn message_of(record: &Value) -> Option<&Map<String, Value>> {
    object_field(record, "message")
}

fn kind_of(record: &Value) -> Option<&str> {
    record.get("type").and_then(Value::as_str)
}

fn is_synthetic(message: &Map<String, Value>) -> bool {
    message.get("model").and_then(Value::as_str) == Some(SYNTHETIC_MODEL)
}

/// `value` when it is a positive integer, else zero.
fn count(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(Value::as_object)
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn tool_errors_in(content: Option<&Value>) -> u64 {
    content.and_then(Value::as_array).map_or(0, |blocks| {
        blocks
            .iter()
            .filter_map(Value::as_object)
            .filter(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_result")
                    && block.get("is_error") == Some(&Value::Bool(true))
            })
            .count() as u64
    })
}

/// The `tool_use` blocks of one assistant message as `(block id, tool name)`, with the fallback id
/// `<message id>#<line>.<block index>` for a block without an id.
fn tool_uses_of(number: usize, message_id: &str, content: Option<&Value>) -> Vec<(String, String)> {
    content
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |blocks| {
            blocks
                .iter()
                .enumerate()
                .filter_map(|(index, block)| {
                    let block = block.as_object()?;
                    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                        return None;
                    }
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .map_or_else(|| format!("{message_id}#{number}.{index}"), str::to_owned);
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_owned();
                    Some((id, name))
                })
                .collect()
        })
}

/// Parses a subagent transcript. `None` when the lines are missing (the file was absent or unreadable).
/// `sidecar_text` is the `agent-<id>.meta.json` text when present; a sidecar that is not a JSON object
/// contributes nothing.
pub fn parse_transcript(
    lines: Option<&[String]>,
    sidecar_text: Option<&str>,
) -> Option<TranscriptStats> {
    let lines = lines?;
    let parsed = records(lines);
    let all: Vec<Value> = parsed.iter().map(|(_, record)| record.clone()).collect();
    let meta = sidecar(sidecar_text);

    let stamps: Vec<&str> = all
        .iter()
        .filter_map(|record| record.get("timestamp").and_then(Value::as_str))
        .collect();
    let started_at = stamps
        .iter()
        .find(|stamp| !stamp.is_empty())
        .or_else(|| stamps.first())
        .map(|stamp| (*stamp).to_owned());
    let ended_at = stamps.last().map(|stamp| (*stamp).to_owned());
    let claude_version = all
        .iter()
        .filter_map(|record| record.get("version").and_then(Value::as_str))
        .next()
        .map(str::to_owned);

    let user_messages: Vec<&Map<String, Value>> = parsed
        .iter()
        .filter(|(_, record)| kind_of(record) == Some("user"))
        .filter_map(|(_, record)| message_of(record))
        .collect();
    let prompt = user_messages
        .first()
        .map(|message| text_of(message.get("content")))
        .unwrap_or_default();
    let tool_errors: u64 = user_messages
        .iter()
        .map(|message| tool_errors_in(message.get("content")))
        .sum();

    let assistant: Vec<(usize, &Map<String, Value>)> = parsed
        .iter()
        .filter(|(_, record)| kind_of(record) == Some("assistant"))
        .filter_map(|(number, record)| message_of(record).map(|message| (*number, message)))
        .filter(|(_, message)| !is_synthetic(message))
        .collect();
    let message_ids: Vec<String> = assistant
        .iter()
        .map(|(number, message)| {
            message
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| format!("line-{number}"), str::to_owned)
        })
        .collect();

    let turn_pairs: Vec<(String, Turn)> = assistant
        .iter()
        .zip(message_ids.iter())
        .map(|((_, message), message_id)| {
            let usage = message
                .get("usage")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let output_tokens = count(usage.get("output_tokens"));
            let model = message
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned);
            (
                message_id.clone(),
                Turn {
                    model,
                    output_tokens,
                    usage,
                },
            )
        })
        .collect();
    let turns = group_in_order(turn_pairs, |current, new| {
        if new.output_tokens >= current.output_tokens {
            new
        } else {
            current.clone()
        }
    });

    let tool_pairs: Vec<(String, String)> = assistant
        .iter()
        .zip(message_ids.iter())
        .flat_map(|((number, message), message_id)| {
            tool_uses_of(*number, message_id, message.get("content"))
        })
        .collect();
    let tool_uses = count_in_order(
        group_in_order(tool_pairs, |_, new| new)
            .into_iter()
            .map(|(_, name)| name),
    );

    let models = count_in_order(turns.iter().filter_map(|(_, turn)| turn.model.clone()));
    let usage = turns
        .iter()
        .fold(Usage::default(), |total, (_, turn)| Usage {
            input_tokens: total.input_tokens + count(turn.usage.get(USAGE_FIELDS[0])),
            output_tokens: total.output_tokens + count(turn.usage.get(USAGE_FIELDS[1])),
            cache_creation_input_tokens: total.cache_creation_input_tokens
                + count(turn.usage.get(USAGE_FIELDS[2])),
            cache_read_input_tokens: total.cache_read_input_tokens
                + count(turn.usage.get(USAGE_FIELDS[3])),
        });

    let prompt_chars = prompt.chars().count();
    Some(TranscriptStats {
        model: most_common(&models),
        models,
        usage,
        turns: turns.len(),
        tool_uses,
        tool_errors,
        prompt_chars,
        task_keys: unique_in_order(markers(&prompt, task_key_pattern())),
        trial_ids: unique_in_order(markers(&prompt, trial_id_pattern())),
        claude_version,
        started_at,
        ended_at,
        requested_model: text_field(&meta, "model"),
        description: text_field(&meta, "description"),
        agent_type: text_field(&meta, "agentType"),
        time: breakdown(&all),
    })
}

/// Every tool call the transcript made, one per distinct `tool_use` id, in first-seen order. A block repeated
/// across the lines of one message collapses to one call (the last occurrence wins). `None` lines give an
/// empty list.
pub fn tool_calls(lines: Option<&[String]>) -> Vec<ToolCall> {
    let Some(lines) = lines else {
        return Vec::new();
    };
    let pairs: Vec<(String, ToolCall)> = records(lines)
        .into_iter()
        .filter(|(_, record)| kind_of(record) == Some("assistant"))
        .filter_map(|(number, record)| {
            let message = message_of(&record)?;
            if is_synthetic(message) {
                return None;
            }
            let message_id = message
                .get("id")
                .and_then(Value::as_str)
                .map_or_else(|| format!("line-{number}"), str::to_owned);
            let blocks = message
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Some(
                blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(index, block)| {
                        let block = block.as_object()?;
                        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                            return None;
                        }
                        let id = block.get("id").and_then(Value::as_str).map_or_else(
                            || format!("{message_id}#{number}.{index}"),
                            str::to_owned,
                        );
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_owned();
                        let input = block
                            .get("input")
                            .filter(|value| value.is_object())
                            .cloned()
                            .unwrap_or_else(|| json!({}));
                        Some((
                            id.clone(),
                            ToolCall {
                                name,
                                input,
                                tool_use_id: id,
                            },
                        ))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect();
    group_in_order(pairs, |_, new| new.clone())
        .into_iter()
        .map(|(_, call)| call)
        .collect()
}

fn sidecar(text: Option<&str>) -> Map<String, Value> {
    text.and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

fn text_field(meta: &Map<String, Value>, key: &str) -> Option<String> {
    meta.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The first capture group of every match, in order.
fn markers(text: &str, pattern: Option<Regex>) -> Vec<String> {
    pattern.map_or_else(Vec::new, |regex| {
        regex
            .captures_iter(text)
            .filter_map(|captures| captures.get(1))
            .map(|found| found.as_str().to_owned())
            .collect()
    })
}

fn task_key_pattern() -> Option<Regex> {
    Regex::new(r"(?m)^[ \t]*Task-ID:[ \t]*([A-Z][A-Z0-9]{0,9}-\d+)\b").ok()
}

fn trial_id_pattern() -> Option<Regex> {
    Regex::new(r"(?m)^[ \t]*Trial-ID:[ \t]*([0-9A-Za-z][0-9A-Za-z-]{7,})").ok()
}
