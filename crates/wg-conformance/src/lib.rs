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
