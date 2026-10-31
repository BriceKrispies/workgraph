//! The shell of `export analytics`: checks the permission, resolves the workspace, checks the
//! dataset name, copies the warehouse sources in one transaction, builds the fact tables and
//! hands the one asked for to the pure mapping of [`crate::spec::export`].
//!
//! These are free functions over `&mut App`, reached from the data feature's `export` group.

use std::path::Path;

use wg_analytics::build;
use wg_archivespec::Dataset;
use wg_runtime::{App, RequestContext};
use wg_types::WgError;

use super::{load, reads, require};
use crate::spec::export::{self, BAD_DATASET, FACT_TABLES};
use crate::spec::query::{self, QueryRequest};

/// What `export` wrote: the path as given, the format, the dataset name and the byte count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exported {
    pub path: String,
    pub format: String,
    pub dataset: String,
    pub bytes: u64,
}

/// `AnalyticsService.export` (`analytics export`): a fact table, or the rows of a metric query
/// when `request` is given, written to `target` as CSV, JSON or Parquet. The permission and the
/// format are checked first, as Python does; the dataset name defaults to the metric.
pub fn export(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    target: &Path,
    format_text: &str,
    dataset: Option<&str>,
    request: Option<&QueryRequest>,
) -> Result<Exported, WgError> {
    require(app, context)?;
    let format = wg_archivespec::parse_format(format_text)?;
    let (data, name) = match request {
        Some(request) => {
            let name = dataset.unwrap_or(&request.metric).to_owned();
            (metric_dataset(app, context, workspace, request)?, name)
        }
        None => {
            let name = dataset.unwrap_or("").to_owned();
            (fact_dataset(app, context, workspace, &name)?, name)
        }
    };
    let written = wg_archive::analytics::export_dataset(&data, format, target)?;
    Ok(Exported {
        path: written.path.display().to_string(),
        format: format_text.to_owned(),
        dataset: name,
        bytes: written.bytes,
    })
}

/// The rows of a metric query as a dataset: the engine that `analytics query` runs, with the
/// default period and no filters beyond the request's own.
pub fn metric_dataset(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    request: &QueryRequest,
) -> Result<Dataset, WgError> {
    query::check_request(request)?;
    require(app, context)?;
    let loaded = load(
        app,
        workspace,
        request.project.as_deref(),
        request.repository.as_deref(),
    )?;
    let facts = build(&loaded.input);
    let engine = query::engine_query(request, loaded.project, loaded.repository)?;
    let answer = query::run_query(
        &loaded.input,
        &facts,
        &loaded.workspace.to_string(),
        &engine,
    )?;
    Ok(export::rows_dataset(&answer.rows))
}

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
