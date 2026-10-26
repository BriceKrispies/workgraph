//! A scratch directory under the system temporary directory, removed when dropped
//! (`tempfile.TemporaryDirectory` in the Python backup code).

use std::path::PathBuf;

use wg_archivespec as spec;
use wg_types::WgError;

pub(crate) struct Scratch {
    pub(crate) path: PathBuf,
}

impl Scratch {
    /// Creates an empty directory whose name starts with `prefix` and is unique to this process.
    pub(crate) fn new(prefix: &str) -> Result<Scratch, WgError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).map_err(|err| spec::io_failure("create", &path, &err))?;
        Ok(Scratch { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
