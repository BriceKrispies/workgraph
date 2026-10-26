//! How a failed command is reported: the text for each stream and the exit code (contracts O02
//! and O05; `fail` and `handle_errors` of `cli/common.py`).

use serde_json::{Map, Value as Json};
use wg_json::{PyStr, Value, log_json, py_float_repr};
use wg_model::py_repr_str;
use wg_render::{Console, cell_len, char_cells};
use wg_types::WgError;
use wg_wire::error::{CLI_ERROR_CODE, error_body, error_envelope};

/// Exit code of an unexpected error and of `fail()`'s default.
pub const EXIT_UNEXPECTED: u8 = 1;

/// Exit code of an interrupted command (`KeyboardInterrupt`).
pub const EXIT_INTERRUPTED: u8 = 130;

/// Exit code of a usage error (Click's, kept by clap).
pub const EXIT_USAGE: u8 = 2;

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
    /// A usage error that Click raises while it parses the options (`BadParameter`): the usage
    /// line and the `Try` line, then the message in a panel. Click writes it as text even with
    /// `--json`.
    Usage {
        /// The usage line after `Usage: `, such as `workgraph task create [OPTIONS] {title}`.
        usage: String,
        /// The command named in the `Try ... --help` line.
        command: String,
        /// The `Invalid value for ...` message.
        message: String,
    },
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
            Self::Usage { .. } => EXIT_USAGE,
            Self::Interrupted => EXIT_INTERRUPTED,
        }
    }
}

/// Where a Click usage error is reported: the usage line after `Usage: ` and the command named
/// in the `Try ... --help` line, as Click prints them for the command that owns the option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub usage: &'static str,
    pub command: &'static str,
}

impl Usage {
    /// Click's `BadParameter` for `option` given `value`, which its type `kind` (`int` or
    /// `float`) cannot read: `Invalid value for '--x': 'v' is not a valid int.`
    #[must_use]
    pub fn invalid_value(self, option: &str, value: &str, kind: &str) -> CliFailure {
        CliFailure::Usage {
            usage: self.usage.to_owned(),
            command: self.command.to_owned(),
            message: format!(
                "Invalid value for '{option}': {} is not a valid {kind}.",
                py_repr_str(value)
            ),
        }
    }
}

/// The text Python's `int()` and `float()` read: surrounding whitespace dropped, and an
/// underscore allowed only between two digits. `None` when an underscore is misplaced.
#[must_use]
pub fn python_number_text(text: &str) -> Option<String> {
    let chars: Vec<char> = text.trim().chars().collect();
    let mut out = String::with_capacity(chars.len());
    for (index, &c) in chars.iter().enumerate() {
        if c != '_' {
            out.push(c);
            continue;
        }
        let digit_before = index
            .checked_sub(1)
            .and_then(|before| chars.get(before))
            .is_some_and(char::is_ascii_digit);
        let digit_after = chars.get(index + 1).is_some_and(char::is_ascii_digit);
        if !(digit_before && digit_after) {
            return None;
        }
    }
    Some(out)
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
        CliFailure::Usage {
            usage,
            command,
            message,
        } => (
            String::new(),
            // typer draws its usage panel on rich's plain console, which on a legacy Windows
            // console is one cell narrower than the width (see `Console::from_env`).
            usage_text(usage, command, message, console.plain_width()),
        ),
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

/// Click's usage error for a panel `width` cells wide: the usage line, the `Try` line, then the
/// message in an ASCII panel titled `Error` (the box Python draws when its output is not UTF-8).
fn usage_text(usage: &str, command: &str, message: &str, width: usize) -> String {
    let inner = width.saturating_sub(4).max(1);
    let mut out = format!(
        "Usage: {usage}
Try '{command} --help' for help.
"
    );
    out.push_str(&format!(
        "+- Error {}+
",
        "-".repeat(width.saturating_sub(10))
    ));
    for line in wrap_cells(message, inner) {
        let padding = inner.saturating_sub(cell_len(&line.chars().collect::<Vec<_>>()));
        out.push_str(&format!(
            "| {line}{} |
",
            " ".repeat(padding)
        ));
    }
    out.push_str(&format!(
        "+{}+
",
        "-".repeat(width.saturating_sub(2))
    ));
    out
}

/// Greedy word wrap of `text` to `width` cells. Words are split at single spaces; a word wider
/// than a line is cut where it no longer fits.
fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_cells = 0;
    for word in text.split(' ') {
        let word_cells = cell_len(&word.chars().collect::<Vec<_>>());
        let gap = usize::from(!current.is_empty());
        if current_cells + gap + word_cells <= width {
            if gap == 1 {
                current.push(' ');
            }
            current.push_str(word);
            current_cells += gap + word_cells;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        current_cells = 0;
        for c in word.chars() {
            let cells = char_cells(c);
            if current_cells + cells > width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_cells = 0;
            }
            current.push(c);
            current_cells += cells;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
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
