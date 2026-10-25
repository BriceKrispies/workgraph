//! Domain errors: a stable code, a message and ordered details, with the exit-code (O05) and
//! HTTP-status mappings as pure functions of the code.

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
