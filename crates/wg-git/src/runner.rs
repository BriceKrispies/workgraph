//! The git runner: starts `git` with an argument vector, the E24 environment and the default
//! timeout, and turns the outcome into text or an error.
//!
//! The argument vector, environment and messages come from `wg_runtimespec::effects`, the pure
//! half of the runtime that defines git effects; this module only starts the process.
//!
//! Every operation in this crate calls [`Git::run_with`] itself rather than through another
//! helper. The purity gate (`purity-report --candidates`) reads one level of calls, so a function
//! that only calls helpers would pass as pure logic; calling the process chokepoint directly keeps
//! each operation visibly impure.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

use wg_gitcore::{GitError, RunOptions, parse_git_version};
use wg_model::{EffectOutcome, GitOp, ProcessOutput};
use wg_proc::{CaptureLimits, CaptureResult, run_capture};
use wg_procspec::ProcessSpec;
use wg_runtimespec::effects::{GIT_TIMEOUT_MS, git_environment, git_outcome, git_spec};

/// What a git call produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    /// The arguments after the `-C` and `-c` prefix.
    pub args: Vec<String>,
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
}

/// A repository (or working tree) to run git in: the Rust `Git(path, timeout=120.0)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Git {
    /// The directory git runs in (`-C`).
    pub path: PathBuf,
    /// The timeout of a call without its own, in milliseconds (`GIT_TIMEOUT_MS` by default).
    pub timeout_ms: u64,
}

impl Git {
    /// Runs `git -C <path> -c core.quotepath=off -c color.ui=never [-c core.longpaths=true] <args>`
    /// with the E24 environment and the given options.
    ///
    /// # Errors
    /// [`GitError::Run`] when git cannot run or times out, [`GitError::Failed`] for a non-zero
    /// exit when `options.check` is set.
    pub fn run_with(&self, args: &[&str], options: RunOptions<'_>) -> Result<GitOutput, GitError> {
        let windows = cfg!(windows);
        let op = GitOp {
            cwd: self.path.display().to_string(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            stdin: options.stdin.map(str::to_owned),
            timeout_ms: Some(options.timeout_ms.unwrap_or(self.timeout_ms)),
        };
        let mut env = git_environment(std::env::vars_os(), windows);
        for (key, value) in options.env {
            env.insert(OsString::from(key), OsString::from(value));
        }
        let spec = git_spec(&op, env, windows);
        let result = run_capture(
            &spec,
            CaptureLimits {
                max_stdout_bytes: usize::MAX,
                max_stderr_bytes: usize::MAX,
            },
            None,
        );
        let EffectOutcome::Git(outcome) = git_outcome(&op, result) else {
            return Err(GitError::Run(
                "git effect produced a non-git outcome".to_owned(),
            ));
        };
        let ProcessOutput {
            exit_code,
            stdout,
            stderr,
        } = outcome.map_err(|error| GitError::Run(error.message))?;
        if options.check && exit_code != 0 {
            return Err(GitError::Failed {
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                returncode: exit_code,
                stderr,
            });
        }
        Ok(GitOutput {
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            returncode: exit_code,
            stdout,
            stderr,
        })
    }

    /// The raw standard output of `git -C <path> <args>` as bytes, or `None` when the call fails
    /// or times out (the Python `Git.show_file` contract).
    pub fn raw_output(&self, args: &[&str]) -> Option<Vec<u8>> {
        let op = GitOp {
            cwd: self.path.display().to_string(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            stdin: None,
            timeout_ms: Some(self.timeout_ms),
        };
        let windows = cfg!(windows);
        let spec = git_spec(&op, git_environment(std::env::vars_os(), windows), windows);
        let result = run_capture(
            &spec,
            CaptureLimits {
                max_stdout_bytes: usize::MAX,
                max_stderr_bytes: usize::MAX,
            },
            None,
        );
        match result {
            Ok(CaptureResult {
                exit_code: Some(0),
                timed_out: false,
                stdout,
                ..
            }) => Some(stdout),
            _ => None,
        }
    }
}

/// The major and minor version of the installed git, `(0, 0)` when it cannot be read.
pub fn git_version() -> (u32, u32) {
    static VERSION: OnceLock<(u32, u32)> = OnceLock::new();
    *VERSION.get_or_init(|| {
        let spec = ProcessSpec {
            env: Some(git_environment(std::env::vars_os(), cfg!(windows))),
            timeout_ms: Some(GIT_TIMEOUT_MS),
            ..ProcessSpec::new(["git", "--version"])
        };
        let result = run_capture(
            &spec,
            CaptureLimits {
                max_stdout_bytes: usize::MAX,
                max_stderr_bytes: usize::MAX,
            },
            None,
        );
        match result {
            Ok(run) => parse_git_version(&String::from_utf8_lossy(&run.stdout)),
            Err(_) => (0, 0),
        }
    })
}
