//! The effect interpreter (section 4.4): performs a decision's effects after its transaction has
//! committed and reports each outcome as an [`EffectResult`] for the flow that asked.
//!
//! Effects run in order, outside any transaction, and one failing does not stop the next: a
//! failure is an outcome like any other. Git runs through `wg-proc` as `integrations/git/cli.py`
//! runs it; agents and checks with the minimal environment; HTTP through the [`HttpClient`] the
//! runtime was given (the clients themselves are `wg-net`'s); `Wake` and `Publish` through the
//! process's [`Bus`].

use std::io::Write as _;
use std::path::Path;

use rpds::VectorSync;
use wg_model::{
    Effect, EffectError, EffectOutcome, EffectResult, FsOp, GitOp, HttpCall, HttpResponse,
};
use wg_proc::{CaptureLimits, LineOptions, run_capture, run_process};
use wg_runtimespec::effects::{
    atomic_temp_path, fs_error, git_environment, git_outcome, git_spec, http_unavailable,
    spawn_outcome, spawn_spec,
};

use crate::bus::Bus;
use crate::stamp::draw;

/// A blocking HTTP client for `Http` effects.
pub trait HttpClient: Send {
    /// Performs the call. An HTTP error status is a response, not an error.
    fn call(&mut self, call: &HttpCall) -> Result<HttpResponse, EffectError>;
}

/// Performs effects. `http` is `None` until a client is plugged in; `Http` effects then fail as
/// unavailable.
#[derive(Default)]
pub struct Interpreter {
    pub http: Option<Box<dyn HttpClient>>,
    pub bus: Bus,
}

impl Interpreter {
    /// Performs every effect in order.
    pub fn perform_all(&mut self, effects: &VectorSync<Effect>) -> Vec<EffectResult> {
        effects.iter().map(|effect| self.perform(effect)).collect()
    }

    /// Performs one effect.
    pub fn perform(&mut self, effect: &Effect) -> EffectResult {
        let outcome = match effect {
            Effect::Git(op) => run_git(op),
            Effect::Spawn(spec) => run_spawn(spec),
            Effect::Http(call) => match self.http.as_mut() {
                Some(client) => EffectOutcome::Http(client.call(call)),
                None => http_unavailable(call),
            },
            Effect::Fs(op) => EffectOutcome::Fs(run_fs(op)),
            Effect::Wake(wake) => {
                self.bus.wake(wake);
                EffectOutcome::Done
            }
            Effect::Publish(event) => {
                self.bus.publish(event);
                EffectOutcome::Done
            }
        };
        tracing::debug!(
            target: "workgraph.runtime.effects",
            effect = ?effect,
            outcome = ?outcome,
            "performed effect"
        );
        EffectResult {
            effect: effect.clone(),
            outcome,
        }
    }
}

/// `Git.run(..., check=False)`: the whole parent environment with E24 applied.
fn run_git(op: &GitOp) -> EffectOutcome {
    let env = git_environment(std::env::vars_os(), cfg!(windows));
    let spec = git_spec(op, env, cfg!(windows));
    git_outcome(op, run_capture(&spec, CaptureLimits::default(), None))
}

/// An agent or a check, its merged output streamed to the effect's log file when it names one.
fn run_spawn(spec: &wg_model::ProcessSpec) -> EffectOutcome {
    let process = spawn_spec(spec, std::env::vars_os());
    let options = LineOptions {
        log_file: spec.log_path.as_ref().map(std::path::PathBuf::from),
        ..LineOptions::default()
    };
    spawn_outcome(run_process(&process, options, None))
}

/// File-system effects. Removing what is already gone succeeds.
fn run_fs(op: &FsOp) -> Result<(), EffectError> {
    match op {
        FsOp::WriteAtomic { path, contents } => write_atomic(Path::new(path), contents.as_bytes())
            .map_err(|err| fs_error(&format!("cannot write {path}"), &err)),
        FsOp::AppendLine { path, line } => append_line(Path::new(path), line)
            .map_err(|err| fs_error(&format!("cannot append to {path}"), &err)),
        FsOp::CreateDir { path } => std::fs::create_dir_all(path)
            .map_err(|err| fs_error(&format!("cannot create {path}"), &err)),
        FsOp::RemoveFile { path } => match std::fs::remove_file(path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                Err(fs_error(&format!("cannot remove {path}"), &err))
            }
            _ => Ok(()),
        },
        FsOp::RemoveDir { path } => match std::fs::remove_dir_all(path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                Err(fs_error(&format!("cannot remove {path}"), &err))
            }
            _ => Ok(()),
        },
    }
}

/// `atomic_write_bytes`: a hidden temporary sibling, flushed to disk, renamed over the target, so
/// readers never see a partial file.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let (_, entropy) = draw().map_err(|err| std::io::Error::other(err.message))?;
    let unique: String = entropy
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let temp = atomic_temp_path(path, &unique);
    let written = std::fs::File::create_new(&temp).and_then(|mut file| {
        file.write_all(data)?;
        file.sync_all()
    });
    match written.and_then(|()| std::fs::rename(&temp, path)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&temp);
            Err(err)
        }
    }
}

/// Appends `line` and a newline, creating the file and its directory.
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(format!("{line}\n").as_bytes())
}
