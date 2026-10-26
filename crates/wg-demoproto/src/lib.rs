//! wg-demoproto: the pure parts of the scripted `workgraph.agent.v1` demo agent.
//!
//! Command-line parsing, request and manifest parsing, field lookup and the error type. Nothing
//! here touches the file system, a process or the clock; `wg-demo-agent` does the I/O.

use std::fmt;
use std::path::PathBuf;

use serde_json::{Value, json};
use wg_procspec::ProcError;

/// The default `--file` target.
pub const DEFAULT_FILE: &str = "agent_change.py";

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

/// Result of the agent's operations.
pub type Result<T> = std::result::Result<T, AgentError>;

/// Parse a JSON request or manifest. Empty text is the empty object, as the Python agent read
/// `stdin.read() or "{}"`.
///
/// # Errors
/// Malformed JSON, or JSON that is not an object.
pub fn parse_object(text: &str) -> Result<Value> {
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
///
/// # Errors
/// A missing key, or a value that is not a string.
pub fn str_at<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str> {
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

/// The `repositories` array of a manifest.
///
/// # Errors
/// A manifest without a `repositories` array.
pub fn repositories(manifest: &Value) -> Result<&Vec<Value>> {
    manifest
        .get("repositories")
        .and_then(Value::as_array)
        .ok_or_else(|| AgentError::Message("KeyError: \"repositories\"".to_owned()))
}
