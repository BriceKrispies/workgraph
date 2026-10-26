//! The error type of git operations and the options of one git call. Pure data with its
//! formatting; the runner that produces them is `wg-git`.

use std::fmt;

/// Why a git operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// git could not run: not on `PATH`, not startable, or timed out. The message is git's
    /// Python-compatible wording.
    Run(String),
    /// git ran and exited non-zero in a checked call.
    Failed {
        args: Vec<String>,
        returncode: i32,
        stderr: String,
    },
    /// A ref name that is not safe to pass to git.
    InvalidRef(String),
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Run(message) => f.write_str(message),
            Self::Failed {
                args,
                returncode,
                stderr,
            } => f.write_str(&crate::failed_message(args, *returncode, stderr)),
            Self::InvalidRef(reference) => write!(f, "invalid git ref '{reference}'"),
        }
    }
}

impl std::error::Error for GitError {}

/// Options of one git call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions<'a> {
    /// Turn a non-zero exit into [`GitError::Failed`].
    pub check: bool,
    /// Data fed to standard input.
    pub stdin: Option<&'a str>,
    /// Overrides the repository's timeout.
    pub timeout_ms: Option<u64>,
    /// Variables set on top of the E24 environment.
    pub env: &'a [(&'a str, &'a str)],
}

impl RunOptions<'static> {
    /// A checked call with no stdin, the repository's timeout and no extra environment.
    pub const CHECKED: Self = Self {
        check: true,
        stdin: None,
        timeout_ms: None,
        env: &[],
    };

    /// An unchecked call: the caller reads the return code.
    pub const UNCHECKED: Self = Self {
        check: false,
        stdin: None,
        timeout_ms: None,
        env: &[],
    };
}

/// Why `install-hook` could not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookError {
    /// The common git directory could not be read.
    Git(GitError),
    /// The hook file could not be read or written; the message names the path.
    Io(String),
}

impl fmt::Display for HookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(error) => write!(f, "{error}"),
            Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for HookError {}
