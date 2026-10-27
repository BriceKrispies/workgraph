//! The pure half of the `agent` command group (`cli/commands/agents.py`): the clap command, the
//! human view of `agent list` and the refusal of an unknown argument placeholder.

use clap::Command;
use wg_agentspec::PLACEHOLDERS;
use wg_clispec::args::{get, items, python};
use wg_clispec::tree::snapshot_command;
use wg_json::Value;
use wg_render::Console;
use wg_types::WgError;
use wg_types::pyval::prefix;

/// The clap command of the `agent` group: the snapshot's own tree.
#[must_use]
pub fn command() -> Command {
    snapshot_command("agent")
}

/// The human view of `agent list`: name, adapter, availability, detail and the capabilities that
/// are on.
#[must_use]
pub fn list_view(console: &Console, rows: &Value) -> String {
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
                prefix(&text(get(row, "detail")), 60),
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

/// `validate_template`'s refusal, as the `ConfigError` Python raises.
#[must_use]
pub fn unknown_placeholder(name: &str) -> WgError {
    let sorted: std::collections::BTreeSet<&str> = PLACEHOLDERS.iter().copied().collect();
    WgError::config(format!(
        "unknown placeholder {{{name}}} in agent argument template"
    ))
    .detail("allowed", sorted)
}
