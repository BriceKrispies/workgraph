//! The run specification and the results.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

/// What to run and how long to wait for it. All durations are milliseconds.
#[derive(Debug, Clone)]
pub struct ProcessSpec {
    /// The program and its arguments. The program is resolved against this process's `PATH`; no
    /// shell is involved.
    pub argv: Vec<String>,
    /// Working directory of the child.
    pub cwd: Option<PathBuf>,
    /// The exact environment of the child (build it with `base_environment`); `None` inherits
    /// this process's whole environment.
    pub env: Option<BTreeMap<OsString, OsString>>,
    /// Terminate the tree once this many milliseconds have passed since the start.
    pub timeout_ms: Option<u64>,
    /// Bytes written to the child's stdin before it is closed; `None` gives it a null stdin.
    pub stdin_data: Option<Vec<u8>>,
    /// How often timeout and cancellation are checked.
    pub poll_interval_ms: u64,
    /// How long to wait for the output pipes to close after the child has ended. If a descendant
    /// keeps them open past this, the tree is terminated and the result is marked incomplete.
    pub drain_timeout_ms: u64,
}

impl ProcessSpec {
    /// A spec for `argv` with no timeout, the inherited environment, a null stdin, a 250 ms poll
    /// interval and a 60 s drain timeout.
    pub fn new<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            cwd: None,
            env: None,
            timeout_ms: None,
            stdin_data: None,
            poll_interval_ms: 250,
            drain_timeout_ms: 60_000,
        }
    }
}

/// A per-line callback.
pub type LineCallback = Box<dyn FnMut(&str) + Send>;

/// How the merged stdout and stderr are consumed.
pub struct LineOptions {
    /// Called with each line (decoded lossily as UTF-8, trailing `\r`/`\n` removed). Panics in it
    /// are ignored.
    pub on_line: Option<LineCallback>,
    /// Appended to, one line at a time and flushed per line. Parent directories are created.
    pub log_file: Option<PathBuf>,
    /// How many of the last lines the result keeps.
    pub tail_lines: usize,
    /// Longer lines are cut at this many bytes.
    pub max_line_bytes: usize,
}

impl Default for LineOptions {
    fn default() -> Self {
        Self {
            on_line: None,
            log_file: None,
            tail_lines: 200,
            max_line_bytes: 1 << 20,
        }
    }
}

/// The outcome of a line-streamed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessResult {
    /// The argument vector actually run, with the resolved program first.
    pub argv: Vec<String>,
    /// The exit code; on POSIX a death by signal `n` is reported as `-n`.
    pub exit_code: Option<i32>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// The timeout fired and the tree was terminated.
    pub timed_out: bool,
    /// The cancellation probe fired and the tree was terminated.
    pub cancelled: bool,
    /// The last lines of output.
    pub tail: Vec<String>,
    /// The number of lines seen.
    pub lines: u64,
    /// The number of lines cut at `max_line_bytes`.
    pub truncated_lines: u64,
    /// The output pipes did not close in time; the tail may be missing the end of the output.
    pub output_incomplete: bool,
}

impl ProcessResult {
    /// Exit code 0 with neither timeout nor cancellation.
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.cancelled
    }
}

/// Byte limits for a captured run.
#[derive(Debug, Clone, Copy)]
pub struct CaptureLimits {
    /// The most stdout bytes kept.
    pub max_stdout_bytes: usize,
    /// The most stderr bytes kept.
    pub max_stderr_bytes: usize,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            max_stdout_bytes: 16 << 20,
            max_stderr_bytes: 1 << 20,
        }
    }
}

/// The outcome of a captured run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    /// The argument vector actually run, with the resolved program first.
    pub argv: Vec<String>,
    /// The exit code; on POSIX a death by signal `n` is reported as `-n`.
    pub exit_code: Option<i32>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// The timeout fired and the tree was terminated.
    pub timed_out: bool,
    /// The cancellation probe fired and the tree was terminated.
    pub cancelled: bool,
    /// The first `max_stdout_bytes` of stdout.
    pub stdout: Vec<u8>,
    /// The first `max_stderr_bytes` of stderr.
    pub stderr: Vec<u8>,
    /// Stdout was longer than the limit.
    pub stdout_truncated: bool,
    /// Stderr was longer than the limit.
    pub stderr_truncated: bool,
    /// The output pipes did not close in time.
    pub output_incomplete: bool,
}

impl CaptureResult {
    /// Exit code 0 with neither timeout nor cancellation.
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.cancelled
    }
}

/// The reported exit code: the real code if there is one, else `-signal` (POSIX death by signal).
pub fn exit_code_from_parts(code: Option<i32>, signal: Option<i32>) -> Option<i32> {
    code.or_else(|| signal.map(|s| -s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(exit_code_from_parts(Some(3), None), Some(3));
        assert_eq!(exit_code_from_parts(None, Some(9)), Some(-9));
        assert_eq!(exit_code_from_parts(None, None), None);
    }

    #[test]
    fn spec_defaults() {
        let spec = ProcessSpec::new(["git", "status"]);
        assert_eq!(spec.argv, ["git", "status"]);
        assert_eq!(
            (
                spec.poll_interval_ms,
                spec.drain_timeout_ms,
                spec.timeout_ms
            ),
            (250, 60_000, None)
        );
    }
}
