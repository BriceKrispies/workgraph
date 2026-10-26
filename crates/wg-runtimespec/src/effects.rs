//! The pure half of the effect interpreter: how an [`Effect`](wg_model::Effect) becomes a process
//! specification, and how what the runner reports becomes an [`EffectOutcome`].
//!
//! * Git runs as `integrations/git/cli.py` runs it: `git -C <cwd> -c core.quotepath=off -c
//!   color.ui=never [-c core.longpaths=true on Windows] <args>`, the whole parent environment with
//!   the E24 variables set and `GIT_DIR`/`GIT_WORK_TREE` removed, a 120 second default timeout,
//!   output decoded as UTF-8 with replacement.
//! * Agents and checks run with the minimal environment of `integrations/process.py` plus the
//!   variables the effect names, stdout and stderr merged and streamed.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;

use wg_model::{
    EffectError, EffectErrorKind, EffectOutcome, GitOp, HttpCall, ProcessExit, ProcessOutput,
};
use wg_procspec::{CaptureResult, ProcError, ProcessResult, ProcessSpec, base_environment_from};

/// `Git(timeout=120.0)`.
pub const GIT_TIMEOUT_MS: u64 = 120_000;

/// E24: the variables set for every git child.
pub const GIT_ENV: [(&str, &str); 6] = [
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_PAGER", "cat"),
    ("PAGER", "cat"),
    ("LC_ALL", "C"),
    ("LANG", "C"),
    ("GIT_OPTIONAL_LOCKS", "0"),
];

/// E24: the variables removed from every git child's environment.
pub const GIT_ENV_REMOVED: [&str; 2] = ["GIT_DIR", "GIT_WORK_TREE"];

/// `Git._env()`: the parent environment with [`GIT_ENV`] set and [`GIT_ENV_REMOVED`] removed
/// (names compared case-insensitively on Windows, where Python's `os.environ` upper-cases them).
pub fn git_environment<P>(parent: P, windows: bool) -> BTreeMap<OsString, OsString>
where
    P: IntoIterator<Item = (OsString, OsString)>,
{
    base_environment_from(parent, GIT_ENV, true)
        .into_iter()
        .filter(|(key, _)| {
            !GIT_ENV_REMOVED.iter().any(|removed| {
                if windows {
                    key.eq_ignore_ascii_case(removed)
                } else {
                    key.as_os_str() == OsStr::new(removed)
                }
            })
        })
        .collect()
}

/// The argument vector of a git effect, program first.
pub fn git_argv(op: &GitOp, windows: bool) -> Vec<String> {
    let longpaths: &[&str] = if windows {
        &["-c", "core.longpaths=true"]
    } else {
        &[]
    };
    [
        "git",
        "-C",
        op.cwd.as_str(),
        "-c",
        "core.quotepath=off",
        "-c",
        "color.ui=never",
    ]
    .iter()
    .chain(longpaths)
    .map(|arg| (*arg).to_owned())
    .chain(op.args.iter().cloned())
    .collect()
}

/// The process specification of a git effect. `env` is [`git_environment`]'s result.
pub fn git_spec(op: &GitOp, env: BTreeMap<OsString, OsString>, windows: bool) -> ProcessSpec {
    ProcessSpec {
        env: Some(env),
        timeout_ms: Some(op.timeout_ms.unwrap_or(GIT_TIMEOUT_MS)),
        stdin_data: op.stdin.as_ref().map(|text| text.clone().into_bytes()),
        ..ProcessSpec::new(git_argv(op, windows))
    }
}

/// `' '.join(args[:n])` of a git effect.
fn leading_args(op: &GitOp, n: usize) -> String {
    op.args
        .iter()
        .take(n)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a git run produced, as the flow sees it. A non-zero exit is an outcome, not an error.
pub fn git_outcome(op: &GitOp, result: Result<CaptureResult, ProcError>) -> EffectOutcome {
    EffectOutcome::Git(match result {
        Ok(run) if run.timed_out => Err(EffectError {
            kind: EffectErrorKind::TimedOut,
            message: format!(
                "git {} timed out after {}s",
                leading_args(op, 3),
                seconds(op.timeout_ms.unwrap_or(GIT_TIMEOUT_MS))
            ),
        }),
        Ok(run) => Ok(ProcessOutput {
            exit_code: run.exit_code.unwrap_or(-1),
            stdout: String::from_utf8_lossy(&run.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&run.stderr).into_owned(),
        }),
        Err(ProcError::NotFound(_)) => Err(EffectError {
            kind: EffectErrorKind::NotFound,
            message: "git executable not found on PATH".to_owned(),
        }),
        Err(err) => Err(EffectError {
            kind: EffectErrorKind::Unavailable,
            message: format!("failed to execute git: {err}"),
        }),
    })
}

/// Python's `str()` of a timeout in seconds given in milliseconds (`120.0`, `0.5`).
fn seconds(ms: u64) -> String {
    // Millisecond counts are far below 2^53, so the conversion is exact.
    #[allow(clippy::cast_precision_loss)]
    let value = ms as f64 / 1000.0;
    wg_json::py_float_repr(value)
}

/// The process specification of an agent or check: the minimal environment of `process.py` with
/// the effect's variables on top.
pub fn spawn_spec<P>(spec: &wg_model::ProcessSpec, parent: P) -> ProcessSpec
where
    P: IntoIterator<Item = (OsString, OsString)>,
{
    ProcessSpec {
        cwd: spec.cwd.as_ref().map(PathBuf::from),
        env: Some(base_environment_from(
            parent,
            spec.env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
            false,
        )),
        timeout_ms: spec.timeout_ms,
        stdin_data: spec.stdin.as_ref().map(|text| text.clone().into_bytes()),
        ..ProcessSpec::new(spec.argv.iter().cloned())
    }
}

/// How an agent or check ended. Output was merged, so its tail is `stdout` and `stderr` is empty.
pub fn spawn_outcome(result: Result<ProcessResult, ProcError>) -> EffectOutcome {
    EffectOutcome::Spawn(match result {
        Ok(run) => Ok(ProcessExit {
            exit_code: run.exit_code,
            timed_out: run.timed_out,
            duration_ms: run.duration_ms,
            stdout: run.tail.join("\n"),
            stderr: String::new(),
        }),
        Err(err) => Err(proc_error(&err)),
    })
}

/// A runner failure as an effect error.
pub fn proc_error(err: &ProcError) -> EffectError {
    EffectError {
        kind: match err {
            ProcError::NotFound(_) | ProcError::EmptyArgv | ProcError::InvalidExecutable => {
                EffectErrorKind::NotFound
            }
            ProcError::Spawn { .. } => EffectErrorKind::Unavailable,
            ProcError::LogFile { .. } | ProcError::Wait(_) => EffectErrorKind::Failed,
        },
        message: err.to_string(),
    }
}

/// A file-system failure as an effect error. `what` names the operation and its path.
pub fn fs_error(what: &str, err: &io::Error) -> EffectError {
    EffectError {
        kind: match err.kind() {
            io::ErrorKind::NotFound => EffectErrorKind::NotFound,
            io::ErrorKind::TimedOut => EffectErrorKind::TimedOut,
            _ => EffectErrorKind::Failed,
        },
        message: format!("{what}: {err}"),
    }
}

/// The outcome of an HTTP effect when the runtime has no client for it.
pub fn http_unavailable(call: &HttpCall) -> EffectOutcome {
    EffectOutcome::Http(Err(EffectError {
        kind: EffectErrorKind::Unavailable,
        message: format!("no HTTP client is configured for {}", call.url),
    }))
}

/// `tempfile.mkstemp(prefix=f".{name}.", suffix=".tmp")`'s name for an atomic write of `path`,
/// with `unique` (drawn by the shell) as the random part.
pub fn atomic_temp_path(path: &std::path::Path, unique: &str) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{unique}.tmp"))
}

#[cfg(test)]
mod tests {
    use rpds::VectorSync;

    use super::*;

    fn op(args: &[&str]) -> GitOp {
        GitOp {
            cwd: "/repo".to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            stdin: None,
            timeout_ms: None,
        }
    }

    fn vars(items: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        items
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    // covers: E24
    #[test]
    fn git_environment_sets_and_removes() {
        let env = git_environment(
            vars(&[("GIT_DIR", "/x"), ("SECRET", "kept"), ("LANG", "en_US")]),
            false,
        );
        assert_eq!(env.get(OsStr::new("LANG")), Some(&OsString::from("C")));
        assert_eq!(env.get(OsStr::new("SECRET")), Some(&OsString::from("kept")));
        assert!(!env.contains_key(OsStr::new("GIT_DIR")));
        assert_eq!(
            env.get(OsStr::new("GIT_PAGER")),
            Some(&OsString::from("cat"))
        );
        let windows = git_environment(vars(&[("Git_Work_Tree", "/x")]), true);
        assert!(
            windows
                .keys()
                .all(|k| !k.eq_ignore_ascii_case("GIT_WORK_TREE"))
        );
    }

    #[test]
    fn git_argv_and_spec() {
        assert_eq!(
            git_argv(&op(&["status"]), false),
            [
                "git",
                "-C",
                "/repo",
                "-c",
                "core.quotepath=off",
                "-c",
                "color.ui=never",
                "status"
            ]
        );
        assert_eq!(
            git_argv(&op(&["log"]), true)[7..],
            ["-c", "core.longpaths=true", "log"]
        );
        let spec = git_spec(&op(&["status"]), BTreeMap::new(), false);
        assert_eq!(spec.timeout_ms, Some(120_000));
    }

    #[test]
    fn git_outcomes() {
        let run = CaptureResult {
            argv: vec![],
            exit_code: Some(1),
            duration_ms: 3,
            timed_out: false,
            cancelled: false,
            stdout: b"out".to_vec(),
            stderr: b"bad".to_vec(),
            stdout_truncated: false,
            stderr_truncated: false,
            output_incomplete: false,
        };
        assert_eq!(
            git_outcome(&op(&["status"]), Ok(run.clone())),
            EffectOutcome::Git(Ok(ProcessOutput {
                exit_code: 1,
                stdout: "out".to_owned(),
                stderr: "bad".to_owned()
            }))
        );
        let timed_out = git_outcome(
            &op(&["fetch", "origin", "main", "--prune"]),
            Ok(CaptureResult {
                timed_out: true,
                ..run
            }),
        );
        assert_eq!(
            timed_out,
            EffectOutcome::Git(Err(EffectError {
                kind: EffectErrorKind::TimedOut,
                message: "git fetch origin main timed out after 120.0s".to_owned()
            }))
        );
        assert!(matches!(
            git_outcome(&op(&[]), Err(ProcError::NotFound("git".to_owned()))),
            EffectOutcome::Git(Err(EffectError {
                kind: EffectErrorKind::NotFound,
                ..
            }))
        ));
    }

    #[test]
    fn spawn_spec_uses_the_minimal_environment() {
        let spec = wg_model::ProcessSpec {
            argv: VectorSync::new_sync().push_back("agent".to_owned()),
            cwd: Some("/w".to_owned()),
            env: VectorSync::new_sync().push_back(("WORKGRAPH_TASK".to_owned(), "WG-1".to_owned())),
            stdin: Some("hi".to_owned()),
            timeout_ms: Some(5),
            log_path: None,
        };
        let built = spawn_spec(&spec, vars(&[("PATH", "/bin"), ("API_TOKEN", "x")]));
        let env = built.env.unwrap_or_default();
        assert!(env.contains_key(OsStr::new("PATH")));
        assert!(!env.contains_key(OsStr::new("API_TOKEN")));
        assert_eq!(
            env.get(OsStr::new("WORKGRAPH_TASK")),
            Some(&OsString::from("WG-1"))
        );
        assert_eq!(built.stdin_data, Some(b"hi".to_vec()));
        assert_eq!(built.cwd, Some(PathBuf::from("/w")));
    }

    #[test]
    fn temp_path_is_a_hidden_sibling() {
        let path = atomic_temp_path(std::path::Path::new("dir/api-token"), "abc");
        assert_eq!(path, PathBuf::from("dir/.api-token.abc.tmp"));
    }
}
