//! The telemetry feature (docs/rust-rewrite.md 4.16, D15): hook ingestion and spool replay, agent
//! sessions and their bindings, subagent runs, hook and tool events, retention, and the Claude
//! Code commands (`claude status, replay, bind, unbind, sessions, mcp-config, backfill-time,
//! backfill-transcripts`) with the status route (H121). The hook payloads, redaction, correlation,
//! transcript parsing and the installer's settings rules are the core crate `wg-telemetry` and the
//! pure [`spec`]; this crate holds the statement text, the readers over a session, the ingestion
//! services over an `App`, and the command and route glue. Modules:
//!
//! * [`spec`] (Pure): the statements and row decoding, the installation decisions, the command
//!   arguments and the human views;
//! * [`service`] (Shell): the session readers (free functions over `&mut Session`), the hook
//!   services and the Claude Code services (free functions over `&mut App`);
//! * [`cli`] (Shell): the clap group, a table of `fn` pointers into [`service`];
//! * [`http`] (Shell): the route table of H121.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod http;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The telemetry feature: the `claude` CLI group and the H121 route. It has no tools, resources
/// or jobs yet.
pub static FEATURE: Feature = Feature {
    name: "telemetry",
    cli: &[cli::GROUP],
    routes: &[http::GROUP],
    tools: &[],
    resources: &[],
    jobs: &[],
};
