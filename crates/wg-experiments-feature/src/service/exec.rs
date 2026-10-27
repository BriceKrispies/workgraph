//! Shell: the process and git helpers of the trial runner and the replay glue. Commands run as
//! argument vectors with the inherited environment plus the protocol's (`{**os.environ, **env}`).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::Value as Json;
use wg_experiments::replay::ReplayConfig;
use wg_git::{Git, GitOutput};
use wg_gitcore::RunOptions;
use wg_proc::{CaptureLimits, run_capture};
use wg_procspec::ProcessSpec;
use wg_runtimespec::effects::GIT_TIMEOUT_MS;
use wg_types::{ErrorCode, WgError};

use crate::spec::replay::git_failure;
use crate::spec::runner::{command_record, python_exit, record_exit};

/// The runner for the repository or working tree at `path` (`Git(path)`).
fn git_at(path: &Path) -> Git {
    Git {
        path: path.to_path_buf(),
        timeout_ms: GIT_TIMEOUT_MS,
    }
}

/// `Git(path).run(*args, ...)` with `options`.
///
/// # Errors
///
/// Returns the `GitError` of a call that cannot run, or exits non-zero in a checked call.
pub fn run(path: &Path, args: &[&str], options: RunOptions<'_>) -> Result<GitOutput, WgError> {
    git_at(path)
        .run_with(args, options)
        .map_err(|error| git_failure(&error))
}

/// `Git(path).out(*args)`: the standard output of a checked call.
///
/// # Errors
///
/// Returns the `GitError` of a call that cannot run or exits non-zero.
pub fn out(path: &Path, args: &[&str]) -> Result<String, WgError> {
    run(path, args, RunOptions::CHECKED).map(|output| output.stdout)
}

/// `Git(path).out(*args, check=False)`: the standard output of an unchecked call.
///
/// # Errors
///
/// Returns the `GitError` of a call that cannot run.
pub fn out_unchecked(path: &Path, args: &[&str]) -> Result<String, WgError> {
    run(path, args, RunOptions::UNCHECKED).map(|output| output.stdout)
}

/// `Git(path).resolve_commit(reference)`.
///
/// # Errors
///
/// Returns the `GitError` of an invalid ref or a ref that does not resolve.
pub fn resolve_commit(path: &Path, reference: &str) -> Result<String, WgError> {
    git_at(path)
        .resolve_commit(reference)
        .map_err(|error| git_failure(&error))
}

/// `Git(path).show_file(sha, file)`: the content of a file at a commit, `None` when it is absent.
#[must_use]
pub fn show_file(path: &Path, sha: &str, file: &str) -> Option<Vec<u8>> {
    git_at(path).show_file(sha, file)
}

/// `Git(path).default_branch()`.
///
/// # Errors
///
/// Returns the `GitError` of a call that cannot run.
pub fn default_branch(path: &Path) -> Result<Option<String>, WgError> {
    git_at(path)
        .default_branch()
        .map_err(|error| git_failure(&error))
}

/// `Git(path).current_branch()`.
///
/// # Errors
///
/// Returns the `GitError` of a call that cannot run.
pub fn current_branch(path: &Path) -> Result<Option<String>, WgError> {
    git_at(path)
        .current_branch()
        .map_err(|error| git_failure(&error))
}

/// `ReplayConfig.for_repository`: the defaults for a Rust and/or Python repository root.
#[must_use]
pub fn config_for(path: &Path) -> ReplayConfig {
    let has = |name: &str| std::fs::metadata(path.join(name)).is_ok_and(|found| found.is_file());
    ReplayConfig::for_manifests(has("Cargo.toml"), has("pyproject.toml"))
}

/// The inherited environment with `extra` on top.
#[must_use]
pub fn environment(extra: &[(String, String)]) -> BTreeMap<OsString, OsString> {
    std::env::vars_os()
        .chain(
            extra
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        )
        .collect()
}

/// `_run_command`: runs one command and records it. A timeout or a command that cannot start
/// records exit -1.
#[must_use]
pub fn run_command(
    argv: &[String],
    cwd: &Path,
    timeout: Option<u64>,
    env: &BTreeMap<OsString, OsString>,
) -> Json {
    let started = Instant::now();
    let mut spec = ProcessSpec::new(argv.iter().cloned());
    spec.cwd = Some(cwd.to_path_buf());
    spec.env = Some(env.clone());
    spec.timeout_ms = timeout;
    let (exit, output) = match run_capture(
        &spec,
        CaptureLimits {
            max_stdout_bytes: usize::MAX,
            max_stderr_bytes: usize::MAX,
        },
        None,
    ) {
        Ok(result) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            let code = match (result.timed_out, result.exit_code) {
                (false, Some(code)) => python_exit(code),
                _ => -1,
            };
            (code, text)
        }
        Err(error) => (-1, error.to_string()),
    };
    command_record(argv, exit, started.elapsed().as_millis(), &output)
}

/// `_exit_code`: the exit code of one command, -1 for a timeout or a command that cannot start.
#[must_use]
pub fn exit_code(
    argv: &[String],
    cwd: &Path,
    timeout: u64,
    env: &BTreeMap<OsString, OsString>,
) -> i64 {
    record_exit(&run_command(argv, cwd, Some(timeout), env))
}

static SCRATCH: AtomicU64 = AtomicU64::new(0);

/// `tempfile.mkdtemp(prefix=...)`: a fresh directory under the system temporary directory.
///
/// # Errors
///
/// Returns a `workgraph_error` when the directory cannot be created.
pub fn scratch_dir(prefix: &str) -> Result<PathBuf, WgError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let name = format!(
        "{prefix}{}-{stamp}-{}",
        std::process::id(),
        SCRATCH.fetch_add(1, Ordering::Relaxed)
    );
    let path = std::env::temp_dir().join(name);
    std::fs::create_dir_all(&path).map_err(|error| {
        WgError::new(
            ErrorCode::WorkgraphError,
            format!("cannot create {}: {error}", path.display()),
        )
    })?;
    Ok(path)
}
