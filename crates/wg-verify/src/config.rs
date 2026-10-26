//! Check configs: one check of a verification configuration ([`CheckSpec`]), validated the way the
//! Python `CheckSpec` model validates it (`verification/configs.py`).
//!
//! A stored config is a JSON object per check. [`CheckSpec::from_json`] accepts exactly the keys
//! Python accepts, applies the same defaults, and refuses a shell interpreter as the executable
//! unless `allow_shell` is set. Verification commands are trusted configuration: they are never
//! parsed out of task descriptions or agent output.

use serde_json::{Map, Value};
use wg_model::entity::JsonObject;
use wg_types::{CheckKind, ErrorCode, WgError};

use crate::signatures::FailureParser;

/// Keys a stored check may carry; any other key is refused, as Pydantic's `extra="forbid"` does.
const SPEC_KEYS: &[&str] = &[
    "name",
    "kind",
    "command",
    "repository",
    "timeout_seconds",
    "required",
    "env",
    "parser",
    "baseline",
    "allow_shell",
    "error_exit_codes",
    "tier",
];

/// Executables that are shell interpreters; refused unless `allow_shell` is set.
const FORBIDDEN_EXECUTABLES: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "cmd",
    "cmd.exe",
    "powershell",
    "pwsh",
    "nu",
];

const NAME_MAX_CHARS: usize = 200;
const TIMEOUT_DEFAULT_SECONDS: f64 = 900.0;
const TIMEOUT_MAX_SECONDS: f64 = 24.0 * 3600.0;

/// How much of a configuration a run covers. Fast checks (lint, format, type check) gate stack
/// children; full checks run once per verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckTier {
    Fast,
    Full,
}

impl CheckTier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Full => "full",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "fast" => Some(Self::Fast),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// `absolute`: the check must pass. `regression`: a failure counts only when the same check also
/// fails on the stack's base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    Absolute,
    Regression,
}

impl Baseline {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absolute => "absolute",
            Self::Regression => "regression",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "absolute" => Some(Self::Absolute),
            "regression" => Some(Self::Regression),
            _ => None,
        }
    }
}

/// One validated check of a verification configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckSpec {
    pub name: String,
    pub kind: CheckKind,
    /// Argument vector; no shell is involved unless `allow_shell` permits one.
    pub command: Vec<String>,
    /// Repository id or name used as the working directory; `None` for integration checks.
    pub repository: Option<String>,
    pub timeout_seconds: f64,
    pub required: bool,
    /// Extra environment for the check, in key order.
    pub env: Vec<(String, String)>,
    pub parser: FailureParser,
    pub baseline: Baseline,
    pub allow_shell: bool,
    /// Exit codes that mean the check could not judge anything: such a run is an error.
    pub error_exit_codes: Vec<i64>,
    pub tier: CheckTier,
}

fn invalid(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}

/// A string field that may be absent or `null`.
fn optional_str(raw: &JsonObject, key: &str) -> Result<Option<String>, WgError> {
    match raw.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(invalid(format!("{key}: expected a string"))),
    }
}

/// An enumerated string field with a default when absent.
fn choice<T>(
    raw: &JsonObject,
    key: &str,
    default: T,
    parse: fn(&str) -> Option<T>,
) -> Result<T, WgError> {
    match optional_str(raw, key)? {
        None => Ok(default),
        Some(text) => parse(&text).ok_or_else(|| invalid(format!("{key}: unknown value {text:?}"))),
    }
}

fn boolean(raw: &JsonObject, key: &str, default: bool) -> Result<bool, WgError> {
    match raw.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(invalid(format!("{key}: expected a boolean"))),
    }
}

/// An environment variable name: `^[A-Z_][A-Z0-9_]{0,63}$`.
fn is_env_name(name: &str) -> bool {
    let first_ok = matches!(name.chars().next(), Some('A'..='Z' | '_'));
    // The first character is ASCII when `first_ok` holds, so byte 1 is a character boundary.
    let rest = name.get(1..).unwrap_or_default();
    first_ok
        && rest.len() <= 63
        && rest
            .chars()
            .all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '_'))
}

fn command_field(raw: &JsonObject) -> Result<Vec<String>, WgError> {
    let items = match raw.get("command") {
        Some(Value::Array(items)) if !items.is_empty() => items,
        Some(Value::Array(_)) => return Err(invalid("command: at least one argument is required")),
        None => return Err(invalid("command: field required")),
        Some(_) => return Err(invalid("command: expected a list of strings")),
    };
    let command = items
        .iter()
        .map(|item| match item {
            Value::String(text) if text.contains('\0') => {
                Err(invalid("command arguments must not contain NUL bytes"))
            }
            Value::String(text) => Ok(text.clone()),
            _ => Err(invalid("command: expected a list of strings")),
        })
        .collect::<Result<Vec<String>, WgError>>()?;
    if command.first().is_some_and(|exe| exe.trim().is_empty()) {
        return Err(invalid("command executable must not be empty"));
    }
    Ok(command)
}

fn env_field(raw: &JsonObject) -> Result<Vec<(String, String)>, WgError> {
    match raw.get("env") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                if !is_env_name(key) {
                    return Err(invalid(format!(
                        "invalid environment variable name {key:?}"
                    )));
                }
                match value {
                    Value::String(text) => Ok((key.clone(), text.clone())),
                    _ => Err(invalid(format!("env {key}: expected a string"))),
                }
            })
            .collect(),
        Some(_) => Err(invalid("env: expected an object of strings")),
    }
}

fn timeout_field(raw: &JsonObject) -> Result<f64, WgError> {
    match raw.get("timeout_seconds") {
        None | Some(Value::Null) => Ok(TIMEOUT_DEFAULT_SECONDS),
        Some(value) => match value.as_f64() {
            Some(seconds) if seconds > 0.0 && seconds <= TIMEOUT_MAX_SECONDS => Ok(seconds),
            _ => Err(invalid(
                "timeout_seconds: must be greater than 0 and at most 86400",
            )),
        },
    }
}

fn exit_codes_field(raw: &JsonObject) -> Result<Vec<i64>, WgError> {
    match raw.get("error_exit_codes") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_i64()
                    .ok_or_else(|| invalid("error_exit_codes: expected a list of integers"))
            })
            .collect(),
        Some(_) => Err(invalid("error_exit_codes: expected a list of integers")),
    }
}

impl CheckSpec {
    /// Validates one stored check object. Errors are [`ErrorCode::ValidationFailed`].
    pub fn from_json(raw: &JsonObject) -> Result<Self, WgError> {
        if let Some(extra) = raw.keys().find(|key| !SPEC_KEYS.contains(&key.as_str())) {
            return Err(invalid(format!("{extra}: extra fields are not permitted")));
        }
        let name = match raw.get("name") {
            Some(Value::String(text)) => text.clone(),
            None => return Err(invalid("name: field required")),
            Some(_) => return Err(invalid("name: expected a string")),
        };
        let name_len = name.chars().count();
        if name_len == 0 || name_len > NAME_MAX_CHARS {
            return Err(invalid("name: length must be between 1 and 200"));
        }
        let spec = Self {
            name,
            kind: choice(raw, "kind", CheckKind::Test, CheckKind::parse)?,
            command: command_field(raw)?,
            repository: optional_str(raw, "repository")?,
            timeout_seconds: timeout_field(raw)?,
            required: boolean(raw, "required", true)?,
            env: env_field(raw)?,
            parser: choice(raw, "parser", FailureParser::Auto, FailureParser::parse)?,
            baseline: choice(raw, "baseline", Baseline::Absolute, Baseline::parse)?,
            allow_shell: boolean(raw, "allow_shell", false)?,
            error_exit_codes: exit_codes_field(raw)?,
            tier: choice(raw, "tier", CheckTier::Full, CheckTier::parse)?,
        };
        spec.check_shell()?;
        Ok(spec)
    }

    /// The model's post-validation: a shell interpreter needs the explicit `allow_shell` opt-in.
    fn check_shell(&self) -> Result<(), WgError> {
        let executable = self.command.first().map_or("", String::as_str);
        let base = executable
            .replace('\\', "/")
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if FORBIDDEN_EXECUTABLES.contains(&base.as_str()) && !self.allow_shell {
            return Err(invalid(format!(
                "check {:?} uses shell interpreter {base:?}; set allow_shell=true to opt in",
                self.name
            )));
        }
        Ok(())
    }

    /// The stored JSON form of this check, with keys in the order Python writes them.
    pub fn to_json(&self) -> Map<String, Value> {
        let entries: [(&str, Value); 12] = [
            ("name", Value::String(self.name.clone())),
            ("kind", Value::String(self.kind.as_str().into())),
            (
                "command",
                Value::Array(self.command.iter().cloned().map(Value::String).collect()),
            ),
            (
                "repository",
                self.repository.clone().map_or(Value::Null, Value::String),
            ),
            ("timeout_seconds", serde_json::json!(self.timeout_seconds)),
            ("required", Value::Bool(self.required)),
            (
                "env",
                Value::Object(
                    self.env
                        .iter()
                        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                        .collect(),
                ),
            ),
            ("parser", Value::String(self.parser.as_str().into())),
            ("baseline", Value::String(self.baseline.as_str().into())),
            ("allow_shell", Value::Bool(self.allow_shell)),
            (
                "error_exit_codes",
                Value::Array(
                    self.error_exit_codes
                        .iter()
                        .copied()
                        .map(Value::from)
                        .collect(),
                ),
            ),
            ("tier", Value::String(self.tier.as_str().into())),
        ];
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }
}
