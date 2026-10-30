//! Shell: the `experiment replay` subgroup (IF-20): `scan` and `create`. The clap tree is the
//! snapshot's own, built by the `experiment` group that owns it; that group's run function hands
//! every `experiment replay ...` invocation to [`run`].

use clap::ArgMatches;
use serde_json::Value as Json;
use wg_iface::cli::args::{many, option_flag, option_list, option_text};
use wg_iface::cli::{CliFailure, Usage, not_implemented_message};
use wg_runtime::{CliState, human_context};

use crate::service::replay::{self, CreateOptions, ScanOptions};
use crate::spec::replay::{
    create_view, env_pairs, float_text, int_text, oracle_template_arg, scan_view,
};

const SCAN: Usage = Usage {
    usage: "workgraph experiment replay scan [OPTIONS]",
    command: "workgraph experiment replay scan",
};

const CREATE: Usage = Usage {
    usage: "workgraph experiment replay create [OPTIONS] SHAS...",
    command: "workgraph experiment replay create",
};

/// `parse_json_arg`: JSON text, or `@file` holding it.
fn json_argument(value: &str, what: &str) -> Result<Json, CliFailure> {
    let source = match value.strip_prefix('@') {
        Some(path) => {
            std::fs::read_to_string(path).map_err(|why| CliFailure::fail(why.to_string(), 1))?
        }
        None => value.to_owned(),
    };
    wg_validate::pyjson::parse_message(&source)
        .map_err(|why| CliFailure::fail(format!("{what} is not valid JSON: {why}"), 2))
}

/// Runs the matched leaf of `experiment replay ...`. `path` is the full path, so `path[2]` is the
/// verb.
pub fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    let repo = option_text(m, "--repo").unwrap_or_default();
    match path.get(2).map(String::as_str) {
        Some("scan") => {
            let rev = option_text(m, "--rev").unwrap_or_else(|| "HEAD".to_owned());
            let options = ScanOptions {
                rev: &rev,
                limit: int_text(option_text(m, "--limit").as_deref(), "--limit", SCAN)?,
                min_lines: int_text(
                    option_text(m, "--min-lines").as_deref(),
                    "--min-lines",
                    SCAN,
                )?
                .unwrap_or(1),
                max_lines: int_text(
                    option_text(m, "--max-lines").as_deref(),
                    "--max-lines",
                    SCAN,
                )?,
                include_excluded: option_flag(m, "--include-excluded"),
            };
            let out = replay::replay_scan(state.app()?, &context, ws, &repo, &options)?;
            state.emit(&out, Some(&|c| scan_view(c, &out)))
        }
        Some("create") => {
            let template = option_text(m, "--oracle-template")
                .map(|text| json_argument(&text, "oracle template"))
                .transpose()?
                .filter(|parsed| !parsed.is_null());
            let oracle_template = template.as_ref().map(oracle_template_arg).transpose()?;
            let env = env_pairs(&option_list(m, "--env"))?;
            let options = CreateOptions {
                validate: option_flag(m, "--validate"),
                oracle_template,
                env,
                timeout: float_text(
                    option_text(m, "--timeout").as_deref(),
                    "--timeout",
                    1800.0,
                    CREATE,
                )?,
            };
            let shas = many(m, "shas");
            let out = replay::replay_create(state.app()?, &context, ws, &repo, &shas, &options)?;
            state.emit(&out, Some(&|c| create_view(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
