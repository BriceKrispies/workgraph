//! Errors of the process runner.

use std::fmt;
use std::io;

/// Why a process could not be started or observed. A child that starts and then fails, times out
/// or is cancelled is not an error: it is reported in the result.
#[derive(Debug)]
pub enum ProcError {
    /// The argument vector was empty.
    EmptyArgv,
    /// The executable name was empty or contained a NUL byte.
    InvalidExecutable,
    /// The executable does not exist (explicit path) or is not on `PATH` (bare name).
    NotFound(String),
    /// The operating system refused to start the process.
    Spawn {
        /// The program as given by the caller.
        program: String,
        /// The underlying error.
        source: io::Error,
    },
    /// The log file could not be opened.
    LogFile {
        /// The log file path.
        path: String,
        /// The underlying error.
        source: io::Error,
    },
    /// Waiting for or terminating the child failed.
    Wait(io::Error),
}

impl fmt::Display for ProcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyArgv => f.write_str("empty argument vector"),
            Self::InvalidExecutable => f.write_str("invalid executable name"),
            Self::NotFound(name) => write!(f, "executable {name:?} was not found"),
            Self::Spawn { program, source } => write!(f, "failed to start {program}: {source}"),
            Self::LogFile { path, source } => write!(f, "cannot open log file {path}: {source}"),
            Self::Wait(source) => write!(f, "failed to wait for the child: {source}"),
        }
    }
}

impl std::error::Error for ProcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. } | Self::LogFile { source, .. } | Self::Wait(source) => {
                Some(source)
            }
            _ => None,
        }
    }
}
