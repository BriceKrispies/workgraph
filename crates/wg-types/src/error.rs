//! Domain errors: a stable code, a message and ordered details, with the exit-code (O05) and
//! HTTP-status mappings as pure functions of the code.
//!
//! Build an error with the constructor named for its code and chain `.detail` for each detail
//! entry; the typical site is one line, like Python's `NotFoundError(msg, details={...})`:
//!
//! ```
//! # use wg_types::WgError;
//! let e = WgError::not_found(format!("task {} not found", 7)).detail("task_id", 7);
//! assert_eq!(e.code.to_string(), "not_found");
//! assert_eq!(e.details["task_id"], 7);
//! ```

use core::fmt;

use serde_json::{Map, Value};

string_enum! {
    /// The machine-readable code of a [`WgError`]. The wire strings are the Python `code` values.
    pub enum ErrorCode {
        WorkgraphError = "workgraph_error",
        NotFound = "not_found",
        ValidationFailed = "validation_failed",
        Conflict = "conflict",
        VersionConflict = "version_conflict",
        InvalidTransition = "invalid_transition",
        DependencyCycle = "dependency_cycle",
        PermissionDenied = "permission_denied",
        ProviderUnavailable = "provider_unavailable",
        GitError = "git_error",
        ConfigError = "config_error",
        ResourceBusy = "resource_busy",
    }
}

impl ErrorCode {
    /// The CLI exit code (contract O05). Codes without a dedicated number exit 1.
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::WorkgraphError | Self::GitError => 1,
            Self::ValidationFailed | Self::DependencyCycle => 2,
            Self::Conflict
            | Self::VersionConflict
            | Self::InvalidTransition
            | Self::ResourceBusy => 3,
            Self::NotFound => 4,
            Self::PermissionDenied => 5,
            Self::ProviderUnavailable => 6,
            Self::ConfigError => 7,
        }
    }

    /// The HTTP status of the error class (contract O06).
    pub const fn http_status(self) -> u16 {
        match self {
            Self::WorkgraphError | Self::ConfigError => 400,
            Self::NotFound => 404,
            Self::ValidationFailed | Self::DependencyCycle => 422,
            Self::Conflict
            | Self::VersionConflict
            | Self::InvalidTransition
            | Self::ResourceBusy => 409,
            Self::PermissionDenied => 403,
            Self::ProviderUnavailable => 503,
            Self::GitError => 500,
        }
    }
}

/// A domain error. Core crates return these; shell crates convert their own errors into one at
/// the runtime boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct WgError {
    pub code: ErrorCode,
    pub message: String,
    /// Ordered details, in insertion order (Python dict order on the wire).
    pub details: Map<String, Value>,
}

impl WgError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: Map::new(),
        }
    }

    /// A `WorkgraphError` error.
    pub fn workgraph(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::WorkgraphError, message)
    }

    /// A `NotFound` error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// A `ValidationFailed` error.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ValidationFailed, message)
    }

    /// A `Conflict` error.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    /// A `VersionConflict` error.
    pub fn version_conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::VersionConflict, message)
    }

    /// A `InvalidTransition` error.
    pub fn invalid_transition(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidTransition, message)
    }

    /// A `DependencyCycle` error.
    pub fn dependency_cycle(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::DependencyCycle, message)
    }

    /// A `PermissionDenied` error.
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message)
    }

    /// A `ProviderUnavailable` error.
    pub fn provider_unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ProviderUnavailable, message)
    }

    /// A `GitError` error.
    pub fn git(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::GitError, message)
    }

    /// A `ConfigError` error.
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ConfigError, message)
    }

    /// A `ResourceBusy` error.
    pub fn resource_busy(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ResourceBusy, message)
    }

    /// Adds one detail entry from any serializable value (what `json!(value)` would build).
    #[must_use]
    pub fn detail(self, key: impl Into<String>, value: impl serde::Serialize) -> Self {
        self.with_detail(key, serde_json::to_value(value).unwrap_or(Value::Null))
    }

    /// Adds one detail entry, keeping insertion order.
    #[must_use]
    pub fn with_detail(self, key: impl Into<String>, value: Value) -> Self {
        let details = self
            .details
            .into_iter()
            .chain(core::iter::once((key.into(), value)))
            .collect();
        Self { details, ..self }
    }

    pub fn exit_code(&self) -> u8 {
        self.code.exit_code()
    }

    pub fn http_status(&self) -> u16 {
        self.code.http_status()
    }
}

impl fmt::Display for WgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for WgError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_the_code_and_details_keep_order() {
        let e = WgError::conflict("busy").detail("b", 1).detail("a", "x");
        assert_eq!(e.code, ErrorCode::Conflict);
        assert_eq!(e.message, "busy");
        assert_eq!(e.details.keys().collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(WgError::validation("v").code, ErrorCode::ValidationFailed);
        assert_eq!(WgError::git("g").code, ErrorCode::GitError);
        assert_eq!(WgError::workgraph("w").code, ErrorCode::WorkgraphError);
    }
}
