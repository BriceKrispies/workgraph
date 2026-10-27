//! `workgraph backup`, `restore`, `export workspace`, `import` (C037, C085, C098, C124): the clap
//! commands come from the snapshot, so help and parameters match `contracts/cli.json`. Each handler
//! reads its matches with [`crate::spec`], calls one [`crate::service`] entry point and prints the
//! result with [`crate::spec`] (the `--json` document, or the human view).
//!
//! `export analytics` (C084) is not served here: it needs the analytics warehouse, which is not
//! ported, so it stays a not-implemented placeholder.

use clap::ArgMatches;
use wg_archive::backup;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{CliGroup, CliState, draw, resolve_paths};

use crate::service;
use crate::spec;

/// The version written to the backup manifest and the export header (`workgraph_version`).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The `backup` command.
pub const BACKUP: CliGroup = CliGroup {
    name: "backup",
    command: spec::backup_command,
    run: run_backup,
};

/// The `restore` command.
pub const RESTORE: CliGroup = CliGroup {
    name: "restore",
    command: spec::restore_command,
    run: run_restore,
};

/// The `export` group: `workspace` here, `analytics` not yet.
pub const EXPORT: CliGroup = CliGroup {
    name: "export",
    command: spec::export_command,
    run: run_export,
};

/// The `import` command.
pub const IMPORT: CliGroup = CliGroup {
    name: "import",
    command: spec::import_command,
    run: run_import,
};

fn run_backup(state: &mut CliState, _path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let output = spec::backup_output(m);
    let include_artifacts = spec::include_artifacts(m);
    let report =
        service::create_backup(state.app()?, output.as_deref(), include_artifacts, VERSION)?;
    let path = report.path.display().to_string();
    let schema = report.schema_revision.as_deref();
    let value = spec::backup_document(
        &path,
        report.bytes,
        &report.stamp,
        VERSION,
        schema,
        include_artifacts,
    );
    state.emit(
        &value,
        Some(&|c| spec::backup_view(c, &path, report.bytes, schema)),
    )
}

/// `restore` runs without the application, as the Python command does, so the store is not opened
/// before the backup replaces it; the clock and the paths are read here.
fn run_restore(state: &mut CliState, _path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let archive = spec::path_arg(m, "archive");
    let paths = resolve_paths(state.home.as_deref())?;
    let (at, _) = draw()?;
    let report =
        backup::restore_backup(&paths, &archive, spec::with_config(m), spec::force(m), at)?;
    let restored_from = report.restored_from.display().to_string();
    let safety = report
        .safety_copy
        .as_ref()
        .map(|path| path.display().to_string());
    let value = spec::restore_document(
        &restored_from,
        &report.manifest,
        safety.as_deref(),
        report.schema_before.as_deref(),
        report.schema_after.as_deref(),
    );
    state.emit(&value, Some(&|c| spec::restore_view(c, &value)))
}

fn run_export(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("workspace") => {
            let output = spec::path_arg(m, "output");
            let reference = state.workspace_ref(spec::workspace_arg(m).as_deref());
            let report =
                service::export_workspace(state.app()?, reference.as_deref(), &output, VERSION)?;
            let written = report.path.display().to_string();
            let tables: Vec<(String, usize)> = report.tables;
            let value = spec::export_document(&written, &tables);
            state.emit(&value, Some(&|c| spec::export_view(c, &written, &tables)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

fn run_import(state: &mut CliState, _path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let source = spec::path_arg(m, "source");
    let report = service::import_workspace(state.app()?, &source)?;
    let value = spec::import_document(&report.workspace_id, &report.tables);
    state.emit(
        &value,
        Some(&|c| spec::import_view(c, &report.workspace_id, &report.tables)),
    )
}
