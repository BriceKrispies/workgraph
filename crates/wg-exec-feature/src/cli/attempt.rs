//! `workgraph attempt list, show, manifests, compare` (IF-09). The clap command is the snapshot's
//! own tree; each read calls one [`crate::service::attempts`] entry point and prints the result
//! with [`crate::spec::cli_attempt`]'s views. The write commands (`create`, `start`, `submit`,
//! `abandon`, `finish`) are not ported yet and answer with the placeholder message.

use clap::ArgMatches;
use wg_clispec::args::{number, option_list, option_text};
use wg_clispec::{CliFailure, Usage, not_implemented_message};
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service::attempts::{self, ListQuery};
use crate::spec::cli_attempt as view;

/// The `attempt` group.
pub const GROUP: CliGroup = CliGroup {
    name: "attempt",
    command: view::command,
    run,
};

/// The usage text `attempt list` reports under a usage error.
const LIST: Usage = Usage {
    usage: "workgraph attempt list [OPTIONS]",
    command: "workgraph attempt list",
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let context = human_context(None)?;
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let task = option_text(m, "--task");
            let statuses = option_list(m, "--status");
            let provider = option_text(m, "--provider");
            let limit = number(m, "--limit", 50, LIST)?;
            let query = ListQuery {
                workspace: workspace.as_deref(),
                task: task.as_deref(),
                statuses: &statuses,
                provider: provider.as_deref(),
                limit,
            };
            let out = attempts::attempt_list(state.app()?, &context, &query)?;
            state.emit(&out, Some(&|c| view::list_view(c, &out)))
        }
        Some("show") => {
            let reference = option_text(m, "attempt_id").unwrap_or_default();
            let out = attempts::attempt_get(state.app()?, &context, &reference)?;
            state.emit(&out, Some(&|c| view::show_view(c, &out)))
        }
        Some("manifests") => {
            let reference = option_text(m, "attempt_id").unwrap_or_default();
            let out = attempts::attempt_manifests(state.app()?, &context, &reference)?;
            state.emit(&out, Some(&|c| view::manifests_view(c, &out)))
        }
        Some("compare") => {
            let first = option_text(m, "first").unwrap_or_default();
            let second = option_text(m, "second").unwrap_or_default();
            let out = attempts::attempt_compare(state.app()?, &context, &first, &second)?;
            state.emit(&out, Some(&|c| view::compare_view(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
