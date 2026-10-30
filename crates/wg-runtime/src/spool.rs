//! The hook spool writer: a record the database could not take (or could not be ingested as sent)
//! is written to the spool directory of the home, for `hook_replay_spool` to ingest later. The
//! file format is `wg_telemetry::spool`'s; this module does the I/O.

use std::path::{Path, PathBuf};

use serde_json::Value;
use wg_telemetry::ingest::Failure;
use wg_telemetry::spool;
use wg_types::WgError;

use crate::app::resolve_paths;
use crate::effects::write_atomic;
use crate::stamp::draw;

/// Writes a spooled record atomically, as a spool file of the home.
pub fn write_spooled(path: &Path, record: &Value) -> Result<(), WgError> {
    write_atomic(path, spool::spool_text(record).as_bytes())
        .map_err(|err| crate::spec::app::io_error("write", path, &err))
}

/// `_spool`: writes the record, with its payload redacted and the reason it could not be ingested, to
/// the spool directory of the home. Returns the file.
pub fn spool_record(
    home: Option<&Path>,
    record: &Value,
    failure: &Failure,
) -> Result<PathBuf, WgError> {
    let paths = resolve_paths(home)?;
    let spool_dir = PathBuf::from(paths.spool_dir().as_str());
    let spooled = spool::spooled(record, &failure.reason());
    let (_, entropy) = draw()?;
    let suffix: String = entropy
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let path = spool_dir.join(spool::spool_name(&spooled, &suffix));
    write_spooled(&path, &spooled)?;
    Ok(path)
}
