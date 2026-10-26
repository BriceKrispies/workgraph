//! `workgraph claude status, replay, bind, unbind, sessions, mcp-config, backfill-time,
//! backfill-transcripts` (C047 to C049 and C052 to C056): the Claude Code commands other than the
//! hook fast path (`claude hook`), `install` and `uninstall`, which their own tasks add.
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler reads its matches with [`crate::spec::args`], calls one [`crate::service::claude`]
//! entry point and prints the result with [`crate::spec::view`].

use clap::ArgMatches;
use serde_json::{Value as Json, json};
use wg_clispec::CliFailure;
use wg_clispec::args::one;
use wg_runtime::{CliGroup, CliState};
use wg_wire::convention::json_value;

use crate::service::claude::{self, ClaudeEnv};
use crate::spec::{args, view};

/// The `claude` group: its clap command is [`args::command`].
pub const GROUP: CliGroup = CliGroup {
    name: "claude",
    command: args::command,
    run,
};

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("status") => {
            let env = ClaudeEnv::from_process();
            let scope = args::scope(m);
            let project = args::project_dir(m);
            let data = claude::status(state.app()?, &env, &scope, project.as_deref())?;
            state.emit(&json_value(&data), Some(&|c| view::status_view(c, &data)))
        }
        Some("replay") => {
            let limit = args::limit(m, 1000, args::REPLAY)?;
            let out = claude::replay(state.app()?, limit)?;
            state.emit(
                &json_value(&out),
                Some(&|c| view::kv_object(c, &out, "replay")),
            )
        }
        Some("bind") => {
            let workspace = state.workspace_ref(one(m, "--workspace").as_deref());
            let session = args::text(m, "--session");
            let cwd = args::cwd(m);
            let task = args::task(m);
            let out = claude::bind(
                state.app()?,
                workspace.as_deref(),
                &task,
                session.as_deref(),
                cwd.as_deref(),
            )?;
            state.emit(&json_value(&out), Some(&|c| view::bind_line(c, &out)))
        }
        Some("unbind") => {
            let binding = args::text(m, "--binding");
            let cwd = args::cwd(m);
            let count = claude::unbind(state.app()?, binding.as_deref(), cwd.as_deref())?;
            let out = json!({"deactivated": count});
            state.emit(&json_value(&out), Some(&|c| view::unbind_line(c, count)))
        }
        Some("sessions") => {
            let limit = args::limit(m, 30, args::SESSIONS)?;
            let rows = claude::sessions(state.app()?, limit)?;
            let out = Json::Array(rows.clone());
            state.emit(&json_value(&out), Some(&|c| view::sessions_view(c, &rows)))
        }
        Some("mcp-config") => {
            let config = claude::mcp_config(args::text(m, "--command").as_deref())?;
            state.emit(
                &json_value(&config),
                Some(&|c| view::mcp_config_view(c, &config)),
            )
        }
        Some("backfill-time") => {
            let limit = args::optional_limit(m, args::BACKFILL_TIME)?;
            let out = claude::backfill_time(state.app()?, limit)?;
            state.emit(
                &json_value(&out),
                Some(&|c| view::kv_object(c, &out, "backfill-time")),
            )
        }
        Some("backfill-transcripts") => {
            let limit = args::optional_limit(m, args::BACKFILL_TRANSCRIPTS)?;
            let out = claude::backfill_transcripts(state.app()?, limit)?;
            state.emit(
                &json_value(&out),
                Some(&|c| view::kv_object(c, &out, "backfill-transcripts")),
            )
        }
        _ => Err(CliFailure::fail(
            wg_clispec::not_implemented_message(path),
            1,
        )),
    }
}
