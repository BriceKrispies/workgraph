//! Why a configuration document could not be read, validated or migrated.

use core::fmt;

use serde_json::Value;
use wg_validate::ValidationError;

use crate::pyfmt::value_repr;

/// A configuration failure: the Python `ConfigError`, or an exception the Python code would raise
/// outside it.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// The document is not valid TOML.
    InvalidToml(String),
    /// The document's `schema_version` is not an integer (`bool`, float and string values are
    /// rejected, as Python's `_schema_version` rejects them). Carries the value as read.
    InvalidSchemaVersion(Value),
    /// The document, or the configuration it produces, fails validation.
    Validation(Vec<ValidationError>),
    /// An exception other than `ConfigError`, named as Python names it.
    Exception(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidToml(detail) => write!(f, "not valid TOML: {detail}"),
            ConfigError::InvalidSchemaVersion(value) => write!(
                f,
                "has an invalid schema_version: expected an integer, got {}",
                value_repr(value)
            ),
            ConfigError::Validation(errors) => {
                write!(f, "failed validation with {} error(s)", errors.len())
            }
            ConfigError::Exception(name) => write!(f, "raised {name}"),
        }
    }
}

impl std::error::Error for ConfigError {}
