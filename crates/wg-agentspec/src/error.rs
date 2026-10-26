//! Errors of the agent adapters.

use std::fmt;
use std::io;

use wg_procspec::ProcError;

use crate::command::UnknownPlaceholder;

/// Why an attempt could not be run. An agent that starts and then fails, times out or is
/// cancelled is not an error: it is reported in the [`AgentRunResult`](crate::AgentRunResult).
#[derive(Debug)]
pub enum AgentError {
    /// The agent cannot be used (`ProviderUnavailableError`): the message says why.
    Unavailable(String),
    /// The configured argument template is invalid (`ConfigError`).
    Config(UnknownPlaceholder),
    /// The process could not be started or observed.
    Proc(ProcError),
    /// A protocol file could not be written.
    Io {
        /// The file.
        path: String,
        /// The underlying error.
        source: io::Error,
    },
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) => f.write_str(message),
            Self::Config(error) => error.fmt(f),
            Self::Proc(error) => error.fmt(f),
            Self::Io { path, source } => write!(f, "cannot write {path}: {source}"),
        }
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Proc(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::Unavailable(_) => None,
        }
    }
}

impl From<ProcError> for AgentError {
    fn from(error: ProcError) -> Self {
        Self::Proc(error)
    }
}
