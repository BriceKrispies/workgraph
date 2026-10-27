//! The automation feature (docs/rust-rewrite.md 4.16, D15): the declarative rules, their trigger
//! runs, dry runs, schedules and notifications. The rule catalog, matching, templates, depth and
//! rate limits and cron are the pure core crate `wg-automation`; this crate holds the rest of the
//! vertical slice, one module per tier:
//!
//! * [`spec`] (Pure): the service as pure decisions (rule create, update and enable; the runs a
//!   trigger, a preview, an event pass and a schedule record), the arguments and human views of the
//!   `automation` commands, the DTOs, the SQL text and the output shapes;
//! * [`service`] (Shell): free functions over `&mut App` and `&mut Session`, the load-decide-apply
//!   glue;
//! * [`cli`] (Shell): the clap group, a table of `fn` pointers into [`service`].
//!
//! The routes (H091 to H100: the automation rules, runs, catalog, dry run and notifications) are
//! [`http`] over [`service`]. The tools (`automation_list`, `automation_trigger` in `wg-mcpspec`)
//! are not served by this crate yet, so the feature has no tool table; its output shapes are in
//! [`spec::shapes`].
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod http;
pub mod jobs;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The automation feature: its CLI group, its routes, its job handler and its maintenance steps.
/// It has no tools or resources yet.
pub static FEATURE: Feature = Feature {
    name: "automation",
    cli: &[cli::GROUP],
    routes: &[http::GROUP],
    tools: &[],
    resources: &[],
    jobs: jobs::JOBS,
    maintenance: jobs::MAINTENANCE,
};
