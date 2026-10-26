//! Running a child whose output lines feed a state machine and an event sink.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use wg_agentspec::{AgentError, AgentEvent};
use wg_proc::{
    Cancel, LineOptions, ProcError, ProcessResult, ProcessSpec, resolve_executable_in, run_process,
};

/// Resolve `name` against this process's `PATH` and `PATHEXT`.
pub(crate) fn resolve(name: &str) -> Result<PathBuf, ProcError> {
    let path = std::env::var_os("PATH");
    let pathext = std::env::var_os("PATHEXT");
    resolve_executable_in(name, path.as_deref(), pathext.as_deref())
}

/// Run `spec`, feeding each output line to `handle(state, line)`, which returns the next state and
/// the events; each event goes to `on_event`. Returns the process result and the final state.
pub(crate) fn run_streamed<S, F, E>(
    spec: &ProcessSpec,
    log_file: PathBuf,
    initial: S,
    handle: F,
    mut on_event: E,
    cancel: Option<Cancel<'_>>,
) -> Result<(ProcessResult, S), AgentError>
where
    S: Default + Send + 'static,
    F: Fn(S, &str) -> (S, Vec<AgentEvent>) + Send + 'static,
    E: FnMut(AgentEvent) + Send + 'static,
{
    let state = Arc::new(Mutex::new(initial));
    let shared = Arc::clone(&state);
    let options = LineOptions {
        on_line: Some(Box::new(move |line: &str| {
            let events = {
                let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                let (next, events) = handle(std::mem::take(&mut *guard), line);
                *guard = next;
                events
            };
            events.into_iter().for_each(&mut on_event);
        })),
        log_file: Some(log_file),
        ..LineOptions::default()
    };
    let process = run_process(spec, options, cancel)?;
    let final_state = std::mem::take(&mut *state.lock().unwrap_or_else(PoisonError::into_inner));
    Ok((process, final_state))
}
