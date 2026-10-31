//! wg-conformance: differential runner, normaliser, drivers and coverage report.
//!
//! This is the runner skeleton: it loads and validates scenario files, runs
//! `cli`, `drain` and `assert` steps against an implementation in a fresh home
//! and temporary directory, resolves placeholders, writes a step log, and has
//! the three modes `diff`, `record` and `check` with a per-file report.
//! `mcp` steps run through the stdio driver in [`mcp`] (CR-04), and `http` steps run
//! through the [`http`] driver (CR-05) against `serve --no-worker`; [`replay`] replays the
//! CF-04 recordings. Database assertions are recognised and skipped until their driver exists.
//! The section 3.4 normaliser lives in [`normalise`] and plugs in through
//! [`compare::Normaliser`].
//!
//! # Recording and checking speed
//!
//! * **Stalls fail fast.** Every wait on an implementation has a no-progress window, set by
//!   `--stall-timeout SECS` (default 10 s; see [`wait`]). A CLI process that prints nothing for
//!   that long is killed; an MCP reply, an HTTP response or a drain's `worker run --once` that
//!   gets no bytes for that long fails its step with a `stalled:` note. The window must be longer
//!   than the longest silent command in the scenarios, so raise it rather than wait out a hang.
//!   The `serve` readiness loop is different: it stops at once when `serve` exits, and otherwise
//!   waits up to [`wait::STARTUP_TIMEOUT`] (90 s) for the health check.
//! * **Use the venv interpreter for the reference.** `--python "uv run --project <wt> workgraph"`
//!   starts `uv` for every CLI step, which re-checks the lock and environment each time. Record
//!   with the checkout's interpreter instead, as a quoted command line:
//!   `--python "<wt>/.venv/Scripts/python.exe -m workgraph"`. Quote any path that contains a space;
//!   [`runner::split_command`] keeps quoted words together and does not treat backslashes as
//!   escapes.
//! * **Run a subset.** `--only id,id,...` (repeatable) or `--list FILE` (one id per line, `#`
//!   comments) runs only the named scenarios. An id that matches no scenario is an error.
//! * **Timing.** The `--report` JSON gives every file a `duration_ms`, and a `slow_steps` list of
//!   the steps that took over a second (`implementation`, `index` (null for setup), `kind`, `ms`).
//!   The text report prints the file's time on its result line.

pub mod compare;
pub mod coverage;
pub mod dbdump;
pub mod gitfix;
pub mod http;
pub mod mcp;
pub mod modes;
pub mod normalise;
pub mod replay;
pub mod runner;
pub mod scenario;
pub mod wait;
