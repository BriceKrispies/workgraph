//! `workgraph agent list, add-command` (IF-09). The clap command is the snapshot's own tree.

use clap::{ArgMatches, Command};
use wg_clispec::args::{get, items, option_flag, option_list, option_text, python, truncate};
use wg_clispec::tree::snapshot_command;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_json::Value;
use wg_render::Console;
use wg_runtime::{CliGroup, CliState, human_context};

use crate::service::agents::{self, CommandAgent};

/// The `agent` group.
pub const GROUP: CliGroup = CliGroup {
    name: "agent",
    command,
    run,
};

fn command() -> Command {
    snapshot_command("agent")
}

fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("list") => {
            let out = agents::agent_list(state.app()?)?;
            state.emit(&out, Some(&|c| list_view(c, &out)))
        }
        Some("add-command") => {
            human_context(None)?;
            let name = option_text(m, "name").unwrap_or_default();
            let executable = option_text(m, "executable").unwrap_or_default();
            let args = option_list(m, "--arg");
            let timeout = match option_text(m, "--timeout") {
                Some(text) => text
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| CliFailure::fail(format!("{text} is not a valid float"), 2))?,
                None => 1800.0,
            };
            let supports_usage = option_flag(m, "--supports-usage");
            let agent = CommandAgent {
                name: &name,
                executable: &executable,
                args: &args,
                timeout_seconds: timeout,
                supports_usage,
            };
            let out = agents::agent_add_command(state.app()?, &agent)?;
            state.emit(
                &out,
                Some(&|c| c.print(&format!("agent {name} configured")).text),
            )
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

/// The human view of `agent list`: name, adapter, availability, detail and the capabilities that
/// are on.
fn list_view(console: &Console, rows: &Value) -> String {
    let cells: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|row| {
            let available = matches!(get(row, "available"), Some(Value::Bool(true)));
            let capabilities: Vec<String> = match get(row, "capabilities") {
                Some(Value::Dict(entries)) => entries
                    .iter()
                    .filter(|(_, on)| matches!(on, Value::Bool(true)))
                    .map(|(key, _)| python(&Value::Str(key.clone())))
                    .collect(),
                _ => Vec::new(),
            };
            vec![
                text(get(row, "name")),
                text(get(row, "adapter")),
                if available { "yes" } else { "NO" }.to_owned(),
                truncate(&text(get(row, "detail")), 60),
                capabilities.join(","),
            ]
        })
        .collect();
    console
        .table(
            &cells,
            &["name", "adapter", "available", "detail", "capabilities"],
            None,
        )
        .text
}

fn text(value: Option<&Value>) -> String {
    value.map(python).unwrap_or_default()
}
