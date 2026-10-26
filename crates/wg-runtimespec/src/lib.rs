//! wg-runtimespec: the pure half of `wg-runtime` (docs/rust-rewrite.md sections 4.1 to 4.5,
//! 4.13).
//!
//! The runtime is the only loop of the system: it stamps each decision, runs load-decide-apply in
//! one transaction, performs the decision's effects and logs what it did. Everything in that work
//! that is a function of values lives here, so the shell crate keeps only the I/O:
//!
//! * [`context`]: [`RequestContext`] (`core/context.py`) and the [`Stamp`](wg_model::Stamp) of
//!   one decision, built from the clock reading and the entropy the shell draws;
//! * [`effects`]: an effect as a process specification (git with the E24 environment, agents and
//!   checks with the minimal one) and the runner's report as an
//!   [`EffectOutcome`](wg_model::EffectOutcome);
//! * [`logs`]: the O14 JSON line of a record and the rotation rule of `<logs>/workgraph.log`;
//! * [`app`]: the API token text, configuration error messages and default-workspace
//!   resolution of the application context (`app.py`).

pub mod app;
pub mod context;
pub mod decisions;
pub mod effects;
pub mod events;
pub mod logs;
pub mod registry;
pub mod settings;
pub mod setup;
pub mod stacks;
pub mod tasks;

pub use context::{GETUSER_VARIABLES, RequestContext, getuser, timestamp_from_unix_nanos};
pub use logs::{Level, LogRecord, format_record};
