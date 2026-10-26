//! wg-conformance: differential runner, normaliser, drivers and coverage report.
//!
//! This is the runner skeleton: it loads and validates scenario files, runs
//! `cli`, `drain` and `assert` steps against an implementation in a fresh home
//! and temporary directory, resolves placeholders, writes a step log, and has
//! the three modes `diff`, `record` and `check` with a per-file report.
//! HTTP steps run through the [`http`] driver (CR-05) against `serve --no-worker`,
//! and [`replay`] replays the CF-04 recordings. MCP steps and database assertions
//! are recognised and skipped until their drivers exist. The section 3.4
//! normaliser lives in [`normalise`] and plugs in through [`compare::Normaliser`].

pub mod compare;
pub mod dbdump;
pub mod gitfix;
pub mod http;
pub mod modes;
pub mod normalise;
pub mod replay;
pub mod runner;
pub mod scenario;
