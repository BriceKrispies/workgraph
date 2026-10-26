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
//! The routes (H091 to H098 of the operation table in `wg-serverspec`) and the tools
//! (`automation_list`, `automation_trigger` in `wg-mcpspec`) are not served by this crate yet, so
//! the feature has no route or tool table; its output shapes are in [`spec::shapes`].
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The automation feature: its CLI group. It has no routes, tools, resources or jobs yet.
pub static FEATURE: Feature = Feature {
    name: "automation",
    cli: &[cli::GROUP],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
};
