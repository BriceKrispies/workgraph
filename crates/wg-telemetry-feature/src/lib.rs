//! The telemetry feature (docs/rust-rewrite.md 4.16, D15): hook ingestion and spool replay, agent
//! sessions and their bindings, subagent runs, hook and tool events, and retention. The hook
//! payloads, redaction, correlation and transcript parsing are the core crate `wg-telemetry`; this
//! crate holds the statement text, the readers over a session and the ingestion services over an
//! `App`. Modules, one per tier:
//!
//! * [`spec`] (Pure): the statements and the row decoding;
//! * [`service`] (Shell): the session readers (free functions over `&mut Session`) and the hook
//!   services (free functions over `&mut App`).
//!
//! The feature exports [`FEATURE`]. The hook services are not served by a route, command or tool
//! yet, so its tables are empty.

pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The telemetry feature. It has no CLI group, routes, tools, resources or jobs yet.
pub static FEATURE: Feature = Feature {
    name: "telemetry",
    cli: &[],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
};
