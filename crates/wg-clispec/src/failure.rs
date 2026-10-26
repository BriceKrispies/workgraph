//! How a failed command is reported: the text for each stream and the exit code (contracts O02
//! and O05; `fail` and `handle_errors` of `cli/common.py`).

use serde_json::{Map, Value as Json};
use wg_json::{PyStr, Value, log_json, py_float_repr};
use wg_render::Console;
use wg_types::WgError;
use wg_wire::error::{CLI_ERROR_CODE, error_body, error_envelope};

/// Exit code of an unexpected error and of `fail()`'s default.
pub const EXIT_UNEXPECTED: u8 = 1;

/// Exit code of an interrupted command (`KeyboardInterrupt`).
pub const EXIT_INTERRUPTED: u8 = 130;

/// What a command ended with when it did not succeed.
#[derive(Clone, Debug, PartialEq)]
pub enum CliFailure {
    /// A domain error: the exit code comes from its class (O05).
    Domain(WgError),
    /// `fail(message, code, details)`: an error the CLI raises itself, code `cli_error`.
    Fail {
        message: String,
        code: u8,
        details: Map<String, Json>,
    },
    /// The command already printed what it had to say; only the exit code is left.
    Exit(u8),
    /// The command was interrupted.
    Interrupted,
}

impl CliFailure {
    /// `fail(message, code)` without details.
    #[must_use]
    pub fn fail(message: impl Into<String>, code: u8) -> Self {
        Self::Fail {
            message: message.into(),
            code,
            details: Map::new(),
        }
    }

    /// The process exit code.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Domain(error) => error.code.exit_code(),
            Self::Fail { code, .. } | Self::Exit(code) => *code,
            Self::Interrupted => EXIT_INTERRUPTED,
        }
    }
}

impl From<WgError> for CliFailure {
    fn from(error: WgError) -> Self {
        Self::Domain(error)
    }
}

/// What to write for a failure, and how to exit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub stdout: String,
    pub stderr: String,
    pub code: u8,
}

/// The report for `failure`. With `--json` the error goes to standard output as one line
/// (`{"error": {...}}`, O02); otherwise a rich-style report goes to standard error.
#[must_use]
pub fn report(failure: &CliFailure, json: bool, console: &Console) -> Report {
    let code = failure.exit_code();
    let (stdout, stderr) = match failure {
        CliFailure::Exit(_) | CliFailure::Interrupted => (String::new(), String::new()),
        CliFailure::Domain(error) if json => (envelope_line(&error_envelope(error)), String::new()),
        CliFailure::Fail {
            message, details, ..
        } if json => (
            envelope_line(&Value::Dict(vec![(
                PyStr::from("error"),
                error_body(CLI_ERROR_CODE, message, details),
            )])),
            String::new(),
        ),
        CliFailure::Domain(error) => {
            let details: Vec<(String, String)> = error
                .details
                .iter()
                .map(|(key, value)| (key.clone(), python_str(value)))
                .collect();
            let pairs: Vec<(&str, &str)> = details
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            let printed = console.error_report(error.code.as_str(), &error.message, &pairs);
            let text = if printed.error.is_some() {
                // The message held text rich reads as broken markup; keep it readable.
                format!("error ({}): {}\n", error.code.as_str(), error.message)
            } else {
                printed.text
            };
            (String::new(), text)
        }
        CliFailure::Fail { message, .. } => {
            let printed = console.fail(message);
            let text = if printed.error.is_some() {
                format!("error: {message}\n")
            } else {
                printed.text
            };
            (String::new(), text)
        }
    };
    Report {
        stdout,
        stderr,
        code,
    }
}

/// `json.dumps(envelope)` plus a newline (O02: compact, Python's default separators).
fn envelope_line(envelope: &Value) -> String {
    let mut line = log_json(envelope).unwrap_or_else(|_| String::from("{\"error\": {}}"));
    line.push('\n');
    line
}

/// Python `str(value)` of a detail value: text as is, everything else as its `repr`.
fn python_str(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        other => python_repr(other),
    }
}

fn python_repr(value: &Json) -> String {
    match value {
        Json::Null => "None".to_owned(),
        Json::Bool(true) => "True".to_owned(),
        Json::Bool(false) => "False".to_owned(),
        Json::Number(number) => {
            if number.is_f64() {
                number.as_f64().map_or_else(String::new, py_float_repr)
            } else {
                number.to_string()
            }
        }
        Json::String(text) => string_repr(text),
        Json::Array(items) => {
            let items: Vec<String> = items.iter().map(python_repr).collect();
            format!("[{}]", items.join(", "))
        }
        Json::Object(map) => {
            let items: Vec<String> = map
                .iter()
                .map(|(key, item)| format!("{}: {}", string_repr(key), python_repr(item)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

/// Python `repr(str)`: single quotes unless the text holds a single quote and no double quote.
fn string_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
