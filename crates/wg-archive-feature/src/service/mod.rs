//! The data entry points of the application: `backup`, `restore`, `export workspace` and `import`.
//!
//! Each one reads the clock and the paths it needs, then calls the shell of `wg-archive` (the zip,
//! the JSON Lines and the store statements), which owns the file formats. These are free functions
//! over `&mut App`, the feature's Shell, reached from [`crate::cli`].

use std::path::Path;

use wg_archive::analytics::{self, AnalyticsExport};
use wg_archive::backup::{self, BackupReport};
use wg_archive::transfer::{self, ExportReport, ImportReport};
use wg_archivespec::Format;
use wg_runtime::{App, RequestContext, draw};
use wg_types::WgError;

/// `backup`: a consistent online copy of the database and the configuration, under `output` or
/// the backups directory. `version` is the stamp written to the manifest.
pub fn create_backup(
    app: &mut App,
    output: Option<&Path>,
    include_artifacts: bool,
    version: &str,
) -> Result<BackupReport, WgError> {
    let (at, _) = draw()?;
    backup::create_backup(
        &app.paths,
        &mut app.runtime.store,
        output,
        include_artifacts,
        at,
        version,
    )
}

/// `export workspace`: every record of one workspace as JSON Lines. `reference` names the
/// workspace by slug or id; without it the default workspace is exported.
pub fn export_workspace(
    app: &mut App,
    reference: Option<&str>,
    output: &Path,
    version: &str,
) -> Result<ExportReport, WgError> {
    let workspace = match reference {
        Some(reference) => app.workspace_id(reference)?,
        None => app.default_workspace_id()?,
    };
    transfer::export_workspace(
        &mut app.runtime.store,
        &workspace.to_string(),
        output,
        version,
    )
}

/// `import`: inserts a workspace export into this database. A workspace that already exists is
/// refused and nothing is written.
pub fn import_workspace(app: &mut App, source: &Path) -> Result<ImportReport, WgError> {
    transfer::import_workspace(&mut app.runtime.store, source)
}

/// `export analytics`: one fact table of `workspace` (the default one when absent) written to
/// `output` as CSV, JSON or Parquet. The fact tables are built by `wg-analytics-feature`.
pub fn export_analytics(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    dataset: &str,
    format: Format,
    output: &Path,
) -> Result<AnalyticsExport, WgError> {
    let rows =
        wg_analytics_feature::service::export::fact_dataset(app, context, workspace, dataset)?;
    analytics::export_dataset(&rows, format, output)
}
