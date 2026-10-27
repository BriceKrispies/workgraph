//! The `mode` of a repository sync (`GitService.sync`): incremental or full. Pure.

use wg_types::{ErrorCode, WgError};

/// `true` for a full reconcile. Any other mode is refused with the message the Python service
/// gives.
pub fn sync_is_full(mode: &str) -> Result<bool, WgError> {
    match mode {
        "incremental" => Ok(false),
        "full" => Ok(true),
        _ => Err(WgError::new(
            ErrorCode::ValidationFailed,
            "mode must be incremental or full",
        )),
    }
}
