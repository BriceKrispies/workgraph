//! Pure: the statement text and row decoding of the experiment family, the human views, the
//! status choices of the `experiment` commands and the group's clap tree.

use clap::Command;
use serde_json::json;
use wg_clispec::Snapshot;
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};
use wg_model::py_repr_str;
use wg_types::WgError;

pub mod lifecycle;
pub mod ops_args;
pub mod ops_view;
pub mod planning;
pub mod predict;
pub mod protocol;
pub mod replay;
pub mod runner;
pub mod runner_sql;
pub mod sql;
pub mod trial;
pub mod view;

/// `ExperimentStatus`, in Python declaration order (the `--status` choices and their message).
pub const STATUSES: [&str; 3] = ["draft", "running", "closed"];

/// The `experiment` command group: the snapshot's tree, with the implemented leaves run by the
/// CLI module.
pub fn command() -> Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "experiment"),
        Err(_) => wg_clispec::tree::command("experiment"),
    }
}

/// `ExperimentService._choice` for the status filter: the value when it is one of [`STATUSES`],
/// else a `validation_failed` error listing the allowed values.
pub fn status_choice(value: &str) -> Result<String, WgError> {
    if STATUSES.contains(&value) {
        Ok(value.to_owned())
    } else {
        let allowed: Vec<serde_json::Value> = STATUSES.iter().map(|s| json!(s)).collect();
        Err(WgError::validation(format!(
            "status {} is not one of: {}",
            py_repr_str(value),
            STATUSES.join(", ")
        ))
        .detail("allowed", serde_json::Value::Array(allowed)))
    }
}
