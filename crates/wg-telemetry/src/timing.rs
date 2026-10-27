//! Where a subagent's wall time went, measured from its transcript (`integrations/claude_code/timing.py`).
//!
//! * a tool call lasts from its assistant `tool_use` to the `tool_result` with the same id;
//! * model time is the gap from the last input the model received to its next assistant message;
//! * waiting is the gap from a turn that ended without a tool call to the next user message that is not a
//!   tool result.
//!
//! Each call is classified by [`TOOL_CLASSES`], a data table: a shell command takes the first class whose
//! command pattern matches it, and any other tool takes the first class listing its name. A call launched
//! with `run_in_background` is recorded under `<class> (launch)`.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use regex::Regex;
use rpds::{RedBlackTreeMap, Vector};
use serde_json::{Map, Value, json};
use wg_types::Timestamp;
use wg_types::pyval::{prefix, text};

/// The version stamped on every breakdown.
pub const TIMING_VERSION: u64 = 1;
/// How many of the longest calls a breakdown lists.
pub const SLOWEST_CALLS: usize = 10;
/// The longest command text a call keeps.
pub const COMMAND_CHARS: usize = 160;
/// The model name Claude Code gives to messages it synthesises itself; they are ignored.
pub const SYNTHETIC_MODEL: &str = "<synthetic>";
/// The class of a shell command no class matches.
pub const OTHER_SHELL: &str = "other shell";
/// The class of any other tool no class names.
pub const OTHER_TOOL: &str = "other tool";
/// The class of a sleep or wait loop.
pub const POLLING: &str = "polling loop";
const SHELL_TOOLS: [&str; 2] = ["Bash", "PowerShell"];

/// One row of the classification table. Commands are case-insensitive regular expressions.
pub struct ToolClass {
    /// The class name a breakdown reports.
    pub name: &'static str,
    /// Tool names that belong to the class.
    pub tools: &'static [&'static str],
    /// Shell command patterns that belong to the class.
    pub commands: &'static [&'static str],
}

/// The classification table. Order matters: the first match wins.
pub const TOOL_CLASSES: &[ToolClass] = &[
    ToolClass {
        name: POLLING,
        tools: &[],
        commands: &[
            r"\bsleep\s+\d",
            r"\bStart-Sleep\b",
            r"\b(until|while)\b[^\n]*\bdo\b",
            r"\bwhile\s*\(",
        ],
    },
    ToolClass {
        name: "cargo test",
        tools: &[],
        commands: &[r"\bcargo\s+test\b"],
    },
    ToolClass {
        name: "cargo build/check",
        tools: &[],
        commands: &[r"\bcargo\s+(build|check|clippy)\b"],
    },
    ToolClass {
        name: "dylint gate",
        tools: &[],
        commands: &[r"dylint"],
    },
    ToolClass {
        name: "rho build/run",
        tools: &[],
        commands: &[r"\brho(\.exe)?\s+(build|run|check|describe)\b"],
    },
    ToolClass {
        name: "program run",
        tools: &[],
        commands: &[r"\.exe\b"],
    },
    ToolClass {
        name: "git",
        tools: &[],
        commands: &[r"^\s*(cd [^&;]*(&&|;)\s*)?git\b"],
    },
    ToolClass {
        name: "search/read",
        tools: &["Read", "Grep", "Glob"],
        commands: &[r"\b(grep|rg|sed -n|cat|head|tail|ls|find|wc)\b"],
    },
    ToolClass {
        name: "edit",
        tools: &["Edit", "Write", "NotebookEdit"],
        commands: &[],
    },
    ToolClass {
        name: "script",
        tools: &[],
        commands: &[r"\b(python|node|powershell|pwsh)\b"],
    },
    ToolClass {
        name: "spawn subagent",
        tools: &["Agent"],
        commands: &[],
    },
    ToolClass {
        name: "background control",
        tools: &["Monitor", "TaskOutput", "TaskStop"],
        commands: &[],
    },
];

/// Every class the table can produce, in table order, including the two fallbacks.
pub fn class_names() -> Vec<&'static str> {
    TOOL_CLASSES
        .iter()
        .map(|class| class.name)
        .chain([OTHER_SHELL, OTHER_TOOL])
        .collect()
}

/// The table with its patterns compiled, for one pass over many calls.
struct Compiled {
    name: &'static str,
    tools: &'static [&'static str],
    commands: Vec<Regex>,
}

fn compile_table() -> Vec<Compiled> {
    TOOL_CLASSES
        .iter()
        .map(|class| Compiled {
            name: class.name,
            tools: class.tools,
            commands: class
                .commands
                .iter()
                .filter_map(|pattern| Regex::new(&format!("(?i){pattern}")).ok())
                .collect(),
        })
        .collect()
}

fn command_text(tool_input: &Value) -> &str {
    text(tool_input.get("command"))
}

fn classify_with(table: &[Compiled], name: &str, tool_input: &Value) -> String {
    if SHELL_TOOLS.contains(&name) {
        let text = command_text(tool_input);
        table
            .iter()
            .find(|class| class.commands.iter().any(|pattern| pattern.is_match(text)))
            .map_or(OTHER_SHELL, |class| class.name)
            .to_owned()
    } else {
        table
            .iter()
            .find(|class| class.tools.contains(&name))
            .map_or(OTHER_TOOL, |class| class.name)
            .to_owned()
    }
}

/// The class of one tool call. Shell commands are matched by their text, every other tool by its name.
pub fn classify(name: &str, tool_input: &Value) -> String {
    classify_with(&compile_table(), name, tool_input)
}

/// A short, single-line label for a call: its command, or its path, pattern, URL or description.
pub fn describe(name: &str, tool_input: &Value) -> String {
    [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "description",
    ]
    .iter()
    .find_map(|key| {
        tool_input
            .get(*key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
    })
    .map_or_else(
        || name.to_owned(),
        |value| {
            prefix(
                &value.split_whitespace().collect::<Vec<_>>().join(" "),
                COMMAND_CHARS,
            )
        },
    )
}

/// Python `round(value, 3)`: correctly rounded, so the formatted text decides the nearest value.
pub fn round3(value: f64) -> f64 {
    format!("{value:.3}").parse::<f64>().unwrap_or(value)
}

/// A timestamp as Python's float epoch seconds: the exact microsecond count divided by a million.
fn epoch(at: Timestamp) -> f64 {
    micros(at) as f64 / 1_000_000.0
}

fn micros(at: Timestamp) -> i128 {
    at.to_offset_datetime().unix_timestamp_nanos() / 1000
}

/// Parses a record timestamp; `Z` means UTC. A naive value (no offset) is the system's local time, as
/// `datetime.timestamp()` reads it, so it is read here with `local_offset_seconds`, the offset of the local
/// zone that the shell reads from the clock. `None` for anything that is not an ISO timestamp, or for a
/// local offset that is not a whole number of minutes.
pub fn parse_timestamp(value: &Value, local_offset_seconds: i32) -> Option<Timestamp> {
    let text = value.as_str()?.replace('Z', "+00:00");
    let has_offset = text.get(10..).is_some_and(|rest| rest.contains(['+', '-']));
    let aware = if has_offset {
        text
    } else {
        format!("{text}{}", offset_text(local_offset_seconds)?)
    };
    Timestamp::parse_iso(&aware).ok()
}

/// The `±HH:MM` text of a UTC offset in seconds, or `None` when it is not a whole number of minutes.
fn offset_text(seconds: i32) -> Option<String> {
    if seconds % 60 != 0 {
        return None;
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    Some(format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60))
}

/// One timed event of a transcript: a user or assistant message with its timestamp.
#[derive(Clone)]
struct Event {
    at: Timestamp,
    assistant: bool,
    message: Map<String, Value>,
}

/// A tool call started and not yet answered.
#[derive(Clone)]
struct Pending {
    start: Timestamp,
    class: String,
    tool: String,
    label: String,
}

/// A finished call.
#[derive(Clone)]
struct Call {
    class: String,
    seconds: f64,
    tool: String,
    command: String,
    at: Timestamp,
}

/// The state folded over the events in time order.
#[derive(Clone)]
struct State {
    pending: RedBlackTreeMap<String, Pending>,
    class_order: Vector<String>,
    class_totals: RedBlackTreeMap<String, (f64, u64)>,
    calls: Vector<Call>,
    model_seconds: f64,
    waiting_seconds: f64,
    last_input: Option<Timestamp>,
    turn_end: Option<Timestamp>,
}

fn initial_state() -> State {
    State {
        pending: RedBlackTreeMap::new(),
        class_order: Vector::new(),
        class_totals: RedBlackTreeMap::new(),
        calls: Vector::new(),
        model_seconds: 0.0,
        waiting_seconds: 0.0,
        last_input: None,
        turn_end: None,
    }
}

/// Seconds from `earlier` to `later`, never negative.
fn gap(later: Timestamp, earlier: Timestamp) -> f64 {
    (epoch(later) - epoch(earlier)).max(0.0)
}

fn blocks(message: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    message
        .get("content")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn block_type(block: &Map<String, Value>) -> Option<&str> {
    block.get("type").and_then(Value::as_str)
}

/// The class recorded for a call: its table class, suffixed when it was launched in the background.
fn launched_class(table: &[Compiled], name: &str, tool_input: &Value) -> String {
    let class = classify_with(table, name, tool_input);
    if tool_input.get("run_in_background") == Some(&Value::Bool(true)) {
        format!("{class} (launch)")
    } else {
        class
    }
}

fn start_calls(
    state: State,
    at: Timestamp,
    message: &Map<String, Value>,
    table: &[Compiled],
) -> State {
    let state = State {
        model_seconds: match state.last_input {
            Some(input) => state.model_seconds + gap(at, input),
            None => state.model_seconds,
        },
        last_input: None,
        ..state
    };
    let uses: Vec<&Map<String, Value>> = blocks(message)
        .into_iter()
        .filter(|block| block_type(block) == Some("tool_use"))
        .collect();
    let state = uses.iter().fold(state, |state, block| {
        let Some(id) = block.get("id").and_then(Value::as_str) else {
            return state;
        };
        let name = block
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let tool_input = block
            .get("input")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        let pending = Pending {
            start: at,
            class: launched_class(table, name, &tool_input),
            tool: name.to_owned(),
            label: describe(name, &tool_input),
        };
        State {
            pending: if state.pending.contains_key(id) {
                state.pending
            } else {
                state.pending.insert(id.to_owned(), pending)
            },
            ..state
        }
    });
    if uses.is_empty() {
        State {
            turn_end: Some(at),
            ..state
        }
    } else {
        state
    }
}

fn finish_call(state: State, at: Timestamp, block: &Map<String, Value>) -> State {
    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
        return state;
    };
    match state.pending.get(id).cloned() {
        None => state,
        Some(started) => {
            let duration = gap(at, started.start);
            let (seconds, count) = state
                .class_totals
                .get(&started.class)
                .copied()
                .unwrap_or((0.0, 0));
            let is_new = !state.class_totals.contains_key(&started.class);
            let class_order = if is_new {
                state.class_order.push_back(started.class.clone())
            } else {
                state.class_order
            };
            let call = Call {
                class: started.class.clone(),
                seconds: round3(duration),
                tool: started.tool,
                command: started.label,
                at: started.start,
            };
            State {
                pending: state.pending.remove(id),
                class_totals: state
                    .class_totals
                    .insert(started.class, (seconds + duration, count + 1)),
                class_order,
                calls: state.calls.push_back(call),
                ..state
            }
        }
    }
}

fn user_message(state: State, at: Timestamp, message: &Map<String, Value>) -> State {
    let results: Vec<&Map<String, Value>> = blocks(message)
        .into_iter()
        .filter(|block| block_type(block) == Some("tool_result"))
        .collect();
    let state = results
        .iter()
        .fold(state, |state, block| finish_call(state, at, block));
    let state = if results.is_empty() {
        match state.turn_end {
            Some(ended) => State {
                waiting_seconds: state.waiting_seconds + gap(at, ended),
                turn_end: None,
                ..state
            },
            None => state,
        }
    } else {
        state
    };
    State {
        last_input: Some(at),
        ..state
    }
}

/// The class names with their rounded seconds and call counts, in first-seen order.
fn class_entries(state: &State) -> Vec<(String, f64, u64)> {
    state
        .class_order
        .iter()
        .filter_map(|class| {
            state
                .class_totals
                .get(class)
                .map(|(seconds, calls)| (class.clone(), *seconds, *calls))
        })
        .collect()
}

/// The time breakdown of one transcript from its parsed records. Non-object records are ignored.
/// `local_offset_seconds` is the local zone's offset, used for naive timestamps (see [`parse_timestamp`]).
pub fn breakdown(records: &[Value], local_offset_seconds: i32) -> Value {
    let table = compile_table();
    let events: BTreeMap<(i128, usize), Event> = records
        .iter()
        .filter_map(Value::as_object)
        .filter(|record| {
            matches!(
                record.get("type").and_then(Value::as_str),
                Some("user" | "assistant")
            )
        })
        .filter_map(|record| {
            let message = record.get("message").and_then(Value::as_object)?;
            if message.get("model").and_then(Value::as_str) == Some(SYNTHETIC_MODEL) {
                return None;
            }
            let at = parse_timestamp(record.get("timestamp")?, local_offset_seconds)?;
            let assistant = record.get("type").and_then(Value::as_str) == Some("assistant");
            Some(Event {
                at,
                assistant,
                message: message.clone(),
            })
        })
        .enumerate()
        .map(|(index, event)| ((micros(event.at), index), event))
        .collect();
    let ordered: Vec<&Event> = events.values().collect();

    let state = ordered.iter().fold(initial_state(), |state, event| {
        if event.assistant {
            start_calls(state, event.at, &event.message, &table)
        } else {
            user_message(state, event.at, &event.message)
        }
    });

    let span = match (ordered.first(), ordered.last()) {
        (Some(first), Some(last)) => gap(last.at, first.at),
        _ => 0.0,
    };
    let classes = class_entries(&state);
    let tool_seconds: f64 = classes.iter().map(|(_, seconds, _)| seconds).sum();
    let longest = slowest(&state.calls);
    json!({
        "version": TIMING_VERSION,
        "span_seconds": round3(span),
        "tool_seconds": round3(tool_seconds),
        "model_seconds": round3(state.model_seconds),
        "waiting_seconds": round3(state.waiting_seconds),
        "classes": classes
            .iter()
            .map(|(class, seconds, calls)| (class.clone(), json!({"seconds": round3(*seconds), "calls": calls})))
            .collect::<Map<String, Value>>(),
        "slowest": longest.iter().map(call_json).collect::<Vec<Value>>(),
    })
}

/// The longest calls, longest first; equal durations keep their order.
fn slowest(calls: &Vector<Call>) -> Vec<Call> {
    calls
        .iter()
        .enumerate()
        .map(|(index, call)| ((Reverse(call.seconds.to_bits()), index), call.clone()))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .take(SLOWEST_CALLS)
        .collect()
}

fn call_json(call: &Call) -> Value {
    json!({
        "class": call.class,
        "seconds": call.seconds,
        "tool": call.tool,
        "command": call.command,
        "at": call.at.format_isoformat(),
    })
}
