//! The error constructors of the browser crates.

use std::fmt::Display;

use wg_types::{ErrorCode, WgError};

/// A browser that is missing, refused the connection or did not answer in time.
#[must_use]
pub fn unavailable(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::ProviderUnavailable, message)
}

/// A local failure while starting the browser or reading and writing its files.
#[must_use]
pub fn io_failure(action: &str, detail: impl Display) -> WgError {
    unavailable(format!("could not {action}: {detail}"))
}
