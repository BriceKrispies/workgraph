//! Effects as data (section 4.4) and the results that come back as flow inputs.
//!
//! A decision lists the effects to perform after its transaction commits; the runtime performs
//! them outside any transaction and feeds each outcome back as an [`EffectResult`], which carries
//! the effect it answers so a flow can match the two. Enqueuing a job is not an effect (it is a
//! `Write` of a `jobs` row) and neither is logging.

use rpds::VectorSync;
use serde_json::Value;
use wg_types::Id;

/// One side effect, performed after commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A git CLI call; the output comes back for `wg-gitcore` to parse.
    Git(GitOp),
    /// A child process: an agent or a check.
    Spawn(ProcessSpec),
    /// An HTTP call to a model provider or a tracker.
    Http(HttpCall),
    /// A file-system change.
    Fs(FsOp),
    /// Nudges local workers after a decision wrote a `jobs` row.
    Wake(JobWake),
    /// Publishes to the SSE subscribers of this process.
    Publish(StreamEvent),
}

/// A git invocation: `git <args>` in `cwd`, with the E24 environment the runner sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOp {
    pub cwd: String,
    pub args: VectorSync<String>,
    /// Data fed to the process's standard input.
    pub stdin: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// A child process. The runner starts it with a minimal environment plus `env`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub argv: VectorSync<String>,
    pub cwd: Option<String>,
    /// Extra environment variables, in the order given.
    pub env: VectorSync<(String, String)>,
    pub stdin: Option<String>,
    pub timeout_ms: Option<u64>,
    /// Where to stream combined output, when it is kept as an artifact.
    pub log_path: Option<String>,
}

/// An HTTP request method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

/// A blocking HTTP call (retries and backoff are the client's, as configured).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpCall {
    pub method: HttpMethod,
    pub url: String,
    /// Request headers, in the order given.
    pub headers: VectorSync<(String, String)>,
    pub body: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// A file-system change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsOp {
    /// Writes the whole file through a temporary file and a rename.
    WriteAtomic {
        path: String,
        contents: String,
    },
    /// Appends one line (a spool record) and a newline.
    AppendLine {
        path: String,
        line: String,
    },
    /// Creates a directory and its parents.
    CreateDir {
        path: String,
    },
    RemoveFile {
        path: String,
    },
    /// Removes a directory tree.
    RemoveDir {
        path: String,
    },
}

/// Wakes local workers that may be idle-waiting for work of `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobWake {
    pub workspace_id: Option<Id>,
    pub kind: String,
}

/// One server-sent event for the subscribers of a workspace's stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    pub workspace_id: Id,
    /// The SSE event name: `domain` or `queue`.
    pub name: String,
    pub data: Value,
}

/// The outcome of one effect, fed back to the flow that asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectResult {
    pub effect: Effect,
    pub outcome: EffectOutcome,
}

/// What performing an effect produced. The variant matches the effect's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectOutcome {
    Git(Result<ProcessOutput, EffectError>),
    Spawn(Result<ProcessExit, EffectError>),
    Http(Result<HttpResponse, EffectError>),
    Fs(Result<(), EffectError>),
    /// Wake and Publish have nothing to report.
    Done,
}

/// The captured output of a short process (git).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// How a long process (an agent or a check) ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessExit {
    /// `None` when the process was killed (timeout or cancellation) before it exited.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// Bounded captures; the full output is in the log file, when one was asked for.
    pub stdout: String,
    pub stderr: String,
}

/// An HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: VectorSync<(String, String)>,
    pub body: String,
}

/// Why an effect could not be performed (a non-zero exit or an HTTP error status is not one:
/// those are outcomes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectError {
    pub kind: EffectErrorKind,
    pub message: String,
}

/// The class of an [`EffectError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectErrorKind {
    /// The program, file or host does not exist.
    NotFound,
    TimedOut,
    /// Connection refused, provider down, executable not runnable.
    Unavailable,
    /// Anything else the shell reports.
    Failed,
}
