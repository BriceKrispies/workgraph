//! wg-procspec: the pure half of the process runner (`wg-proc`).
//!
//! The run specification, the results, the error type, the minimal-environment filter and the
//! executable-suffix rule. Nothing here starts a process or touches the file system; durations are
//! plain milliseconds so no clock type is needed.

mod env;
mod error;
mod resolve;
mod spec;

pub use env::{SAFE_ENV_KEYS, base_environment_from};
pub use error::ProcError;
pub use resolve::{executable_suffixes, validate_executable_name};
pub use spec::{
    CaptureLimits, CaptureResult, LineCallback, LineOptions, ProcessResult, ProcessSpec,
    exit_code_from_parts,
};
