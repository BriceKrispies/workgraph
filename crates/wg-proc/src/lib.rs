//! wg-proc: process spawning, bounded capture, tree termination.
//!
//! The Rust counterpart of `workgraph/integrations/process.py`:
//!
//! * argument vectors only, never a shell ([`ProcessSpec::argv`]);
//! * a minimal child environment ([`base_environment`]);
//! * executable resolution with `PATHEXT` on Windows ([`resolve_executable`]);
//! * stdin feeding, merged stdout and stderr streamed line by line to a callback and a log file,
//!   with only the last lines kept in memory ([`run_process`]);
//! * separate, byte-bounded stdout and stderr capture ([`run_capture`]);
//! * timeouts and cooperative cancellation that terminate the whole process tree: a Job Object on
//!   Windows, a process group on POSIX.
//!
//! This crate is one of the two that may use `unsafe` (FD-01); every block is documented.

mod env;
mod resolve;
mod run;
mod sink;
mod tree;

pub use env::base_environment;
pub use resolve::{resolve_executable, resolve_executable_in};
pub use run::{Cancel, run_capture, run_process};
pub use wg_procspec::{
    CaptureLimits, CaptureResult, LineOptions, ProcError, ProcessResult, ProcessSpec,
    SAFE_ENV_KEYS, base_environment_from,
};
