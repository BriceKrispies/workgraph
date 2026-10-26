//! The data feature (docs/rust-rewrite.md 4.16, D15): `backup`, `restore`, `export workspace` and
//! `import`. The file formats, the zip and the store statements are the shell crate `wg-archive`
//! (F10, F14) over the pure `wg-archivespec`; this crate holds the vertical slice of the CLI, one
//! module per tier:
//!
//! * [`spec`] (Pure): the clap commands, the readers of their matches, the `--json` documents and
//!   the human views;
//! * [`service`] (Shell): free functions over `&mut App`, the clock and the `wg-archive` calls;
//!   `restore` runs without the application, so its glue sits in [`cli`];
//! * [`cli`] (Shell): the clap groups, one [`wg_runtime::CliGroup`] per top-level name.
//!
//! `export analytics` is not served (its dataset needs the analytics warehouse). The feature has no
//! routes or tools, so its `routes` and `tools` tables are empty.
//!
//! The feature exports [`FEATURE`]; the binary collects it into its ordered feature list.

pub mod cli;
pub mod service;
pub mod spec;

use wg_runtime::Feature;

/// The data feature: its CLI groups, `backup`, `restore`, `export` and `import`.
pub static FEATURE: Feature = Feature {
    name: "archive",
    cli: &[cli::BACKUP, cli::RESTORE, cli::EXPORT, cli::IMPORT],
    routes: &[],
    tools: &[],
    resources: &[],
    jobs: &[],
};
