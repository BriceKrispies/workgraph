//! `workgraph claude status, replay, bind, unbind, sessions, mcp-config, backfill-time,
//! backfill-transcripts` (C047 to C049 and C052 to C056) and `install`, `uninstall` (C051, C057):
//! the Claude Code commands other than the hook fast path (`claude hook`), which its own task adds.
//!
//! The clap command is the snapshot's own tree, so help and parameters match `contracts/cli.json`;
//! each handler reads its matches with [`crate::spec::args`], calls one [`crate::service::claude`]
//! entry point and prints the result with [`crate::spec::view`].

use clap::ArgMatches;
use serde_json::{Value as Json, json};
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::one;
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
        Some("install") => {
            let scope = args::scope(m);
            if !matches!(scope.as_str(), "user" | "project" | "local") {
                return Err(CliFailure::fail("scope must be user, project or local", 2));
            }
            // Python ensures the storage exists before installing, so hooks can ingest at once.
            state.app()?;
            let env = ClaudeEnv::from_process();
            let project = args::project_dir(m);
            let events = wg_iface::cli::args::many(m, "--event");
            let command = args::text(m, "--command");
            let mcp = wg_iface::cli::args::flag(m, "--mcp");
            let http = if wg_iface::cli::args::flag(m, "--mcp-http") {
                Some(http_registration(state)?)
            } else {
                None
            };
            let out = claude::install(
                &env,
                &scope,
                project.as_deref(),
                &events,
                command.as_deref(),
                mcp,
                http.as_ref(),
            )?;
            state.emit(
                &json_value(&out),
                Some(&|c| view::kv_object(c, &out, "claude install")),
            )
        }
        Some("uninstall") => {
            let env = ClaudeEnv::from_process();
            let scope = args::scope(m);
            let project = args::project_dir(m);
            let out = claude::uninstall(&env, &scope, project.as_deref())?;
            state.emit(
                &json_value(&out),
                Some(&|c| view::kv_object(c, &out, "claude uninstall")),
            )
        }
        _ => Err(CliFailure::fail(
            wg_iface::cli::not_implemented_message(path),
            1,
        )),
    }
}

/// The endpoint and token `install --mcp-http` registers: the daemon's `/mcp/` on the configured
/// address, with the bearer token from the API token file.
fn http_registration(state: &mut CliState) -> Result<claude::McpHttp, CliFailure> {
    let settings = wg_iface::http::serve::Settings::from_config(&state.app()?.config()?);
    let token_file = state.app()?.paths.api_token_file().to_string();
    let token = std::fs::read_to_string(&token_file).map_err(|err| {
        CliFailure::fail(
            format!(
                "no API token at {token_file} ({err}); start the daemon once with `workgraph serve --ensure`"
            ),
            1,
        )
    })?;
    Ok(claude::McpHttp {
        url: format!("http://{}:{}/mcp/", settings.host, settings.port),
        token: token.trim().to_owned(),
    })
}
