//! The registry feature (docs/rust-rewrite.md 4.16, D15): workspaces, projects, repositories, the
//! settings and setup commands (`init`, `doctor`, `config`), and the registry tools. The pure
//! decisions over workspaces, projects and repositories are the core crate `wg-registry`; this
//! crate is the feature above `wg-runtime`:
//!
//! * [`spec`] (Pure): the request and reply rules, the DTOs, the SQL text, the output shapes and
//!   the clap commands and views;
//! * [`service`] (Shell): free functions over `&mut App` and `&mut Session`, the load-decide-apply
//!   glue;
//! * [`cli`], [`http`] and [`mcp`] (Shell): the clap groups, the settings routes and the tool
//!   table, each a table of `fn` pointers into [`service`].
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod http;
pub mod mcp;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The registry feature: its command groups, its settings routes and its tools. It has no
/// jobs. Its resources are [`mcp::RESOURCES`].
pub static FEATURE: Feature = Feature {
    name: "registry",
    cli: &[
        cli::registry::WORKSPACE,
        cli::registry::PROJECT,
        cli::repo::GROUP,
        cli::setup::CONFIG,
        cli::setup::DOCTOR,
        cli::setup::INIT,
    ],
    routes: &[
        http::GROUP,
        http::PROVIDER_GROUP,
        http::PROJECT_GROUP,
        http::REPOSITORY_GROUP,
    ],
    tools: mcp::TOOLS,
    resources: mcp::RESOURCES,
    jobs: &[],
    maintenance: &[],
};
