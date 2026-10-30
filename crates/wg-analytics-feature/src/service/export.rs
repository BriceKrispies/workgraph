//! The shell of `export analytics`: checks the permission, resolves the workspace, checks the
//! dataset name, copies the warehouse sources in one transaction, builds the fact tables and
//! hands the one asked for to the pure mapping of [`crate::spec::export`].
//!
//! These are free functions over `&mut App`, reached from the data feature's `export` group.

use wg_analytics::build;
use wg_archivespec::Dataset;
use wg_runtime::{App, RequestContext};
use wg_types::WgError;

use super::{reads, require};
use crate::spec::export::{self, BAD_DATASET, FACT_TABLES};

/// The dataset `export analytics --dataset` names, for `workspace` (the default one when absent).
/// Python's order of checks: the permission, the workspace, then the dataset name.
pub fn fact_dataset(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    dataset: &str,
) -> Result<Dataset, WgError> {
    require(app, context)?;
    let ws = app.workspace_or_default(workspace)?;
    if !FACT_TABLES.contains(&dataset) {
        return Err(WgError::validation(BAD_DATASET));
    }
    let input = app.runtime.store.read(reads::analytics_input)?;
    let facts = build(&input);
    Ok(export::fact_dataset(
        &facts,
        &input,
        dataset,
        &ws.to_string(),
    ))
}
