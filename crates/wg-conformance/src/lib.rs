//! wg-conformance: differential runner, normaliser, drivers and coverage report.
//!
//! This is the runner skeleton: it loads and validates scenario files, runs
//! `cli`, `drain` and `assert` steps against an implementation in a fresh home
//! and temporary directory, resolves placeholders, writes a step log, and has
//! the three modes `diff`, `record` and `check` with a per-file report.
//! MCP and HTTP steps and database assertions are recognised and skipped until
//! their drivers exist; the full normaliser is a later task and plugs in through
//! [`compare::Normaliser`].

pub mod compare;
pub mod modes;
pub mod runner;
pub mod scenario;
