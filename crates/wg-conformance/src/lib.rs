//! wg-conformance: differential runner, normaliser, drivers and coverage report.
//!
//! This is the runner skeleton: it loads and validates scenario files, runs
//! `cli`, `drain` and `assert` steps against an implementation in a fresh home
//! and temporary directory, resolves placeholders, writes a step log, and has
//! the three modes `diff`, `record` and `check` with a per-file report.
//! `mcp` steps run through the stdio driver in [`mcp`] (CR-04); `http` steps and database
//! assertions are recognised and skipped until their drivers exist. The section 3.4 normaliser lives in [`normalise`] and
//! plugs in through [`compare::Normaliser`].

pub mod compare;
pub mod coverage;
pub mod dbdump;
pub mod gitfix;
pub mod mcp;
pub mod modes;
pub mod normalise;
pub mod runner;
pub mod scenario;
