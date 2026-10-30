//! Analytics file exports (F15, DV6): `analytics export --format csv|json|parquet`.
//!
//! The datasets are the analytics fact tables and metric results, which `wg-analytics` computes;
//! that crate is not ported yet, so this module takes a [`Dataset`] that the caller has built and
//! writes it in one of the three formats.

use std::path::{Path, PathBuf};

use wg_archivespec::{Dataset, Format};
use wg_types::WgError;

/// What `export_dataset` wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalyticsExport {
    pub path: PathBuf,
    pub format: Format,
    pub bytes: u64,
}

/// Writes a dataset to `target` in `format`, creating its directory.
pub fn export_dataset(
    dataset: &Dataset,
    format: Format,
    target: &Path,
) -> Result<AnalyticsExport, WgError> {
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|err| wg_archivespec::io_failure("create", parent, &err))?;
    }
    let bytes_out = wg_archivespec::dataset_bytes(dataset, format)?;
    std::fs::write(target, bytes_out)
        .map_err(|err| wg_archivespec::io_failure("write", target, &err))?;
    let bytes = std::fs::metadata(target)
        .map_err(|err| wg_archivespec::io_failure("stat", target, &err))?
        .len();
    Ok(AnalyticsExport {
        path: target.to_path_buf(),
        format,
        bytes,
    })
}
