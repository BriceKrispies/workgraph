//! `workgraph attempt list, show, manifests, compare` (IF-09). The clap command is the snapshot's
//! own tree; each read calls one [`crate::service::attempts`] entry point and prints the result
//! with [`crate::spec::cli_attempt`]'s views. The write commands (`create`, `start`, `submit`,
//! `abandon`, `finish`) are not ported yet and answer with the placeholder message.

use clap::ArgMatches;
use wg_clispec::args::{number, option_flag, option_list, option_text};
use wg_clispec::{CliFailure, Usage, not_implemented_message};
use wg_runtime::{CliGroup, CliState, human_context};

use wg_types::AttemptStatus;

use crate::service::attempt_runs::{self as runs, CreateRequest, StartRequest};
use crate::service::attempts::{self, ListQuery};
use crate::service::conclude;
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
        Some("create") => {
            let task = option_text(m, "task").unwrap_or_default();
            let provider = option_text(m, "--provider");
            let model = option_text(m, "--model");
            let session = option_text(m, "--session-id");
            let request = CreateRequest {
                workspace: workspace.as_deref(),
                task: task.trim(),
                provider: provider.as_deref().map_or("external", str::trim),
                adapter: None,
                agent_id: None,
                model: model.as_deref().map(str::trim),
                session_id: session.as_deref().map(str::trim),
                parent_attempt: None,
                status: AttemptStatus::Running,
            };
            let out = runs::attempt_create(state.app()?, &context, &request)?;
            state.emit(&out, Some(&|c| view::create_line(c, &out)))
        }
        Some("start") => {
            let task = option_text(m, "task").unwrap_or_default();
            let agent_name = option_text(m, "--agent-name");
            let adapter = option_text(m, "--adapter");
            let model = option_text(m, "--model");
            let isolation = option_text(m, "--isolation");
            let request = StartRequest {
                workspace: workspace.as_deref(),
                task: &task,
                agent_name: agent_name.as_deref(),
                adapter: adapter.as_deref().unwrap_or("claude_code_subagent"),
                model: model.as_deref(),
                isolation: isolation.as_deref(),
            };
            let out = runs::attempt_start(state.app()?, &context, &request)?;
            state.emit(&out, Some(&|c| view::start_view(c, &out)))
        }
        Some("abandon") => {
            let reference = option_text(m, "attempt").unwrap_or_default();
            let reason = option_text(m, "--reason")
                .unwrap_or_else(|| "agent stopped without submitting".to_owned());
            let out = runs::attempt_abandon(state.app()?, &context, &reference, &reason)?;
            state.emit(&out, Some(&|c| view::abandon_line(c, &out)))
        }
        Some("submit") => {
            let reference = option_text(m, "attempt").unwrap_or_default();
            let summary = option_text(m, "--summary");
            let error = option_text(m, "--error");
            let out = conclude::attempt_submit(
                state.app()?,
                &context,
                &mut conclude::Live,
                &reference,
                option_flag(m, "--failed"),
                summary.as_deref(),
                error.as_deref(),
            )?;
            state.emit(&out, Some(&|c| view::submit_view(c, &reference, &out)))
        }
        Some("finish") => {
            let reference = option_text(m, "attempt_id").unwrap_or_default();
            let status = option_text(m, "status").unwrap_or_default();
            let summary = option_text(m, "--summary").unwrap_or_default();
            let error = option_text(m, "--error");
            let out = runs::attempt_finish(
                state.app()?,
                &context,
                &reference,
                &status,
                &summary,
                error.as_deref(),
            )?;
            state.emit(&out, Some(&|c| view::finish_line(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
