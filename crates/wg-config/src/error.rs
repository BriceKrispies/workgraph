//! Why a configuration document could not be read, validated or migrated.

use core::fmt;

use wg_validate::ValidationError;

/// A configuration failure: the Python `ConfigError`, or an exception the Python code would raise
/// outside it.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// The document is not valid TOML.
    InvalidToml(String),
    /// The document, or the configuration it produces, fails validation.
    Validation(Vec<ValidationError>),
    /// An exception other than `ConfigError`, named as Python names it (`ValueError` when
    /// `schema_version` is not a number, for example).
    Exception(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::InvalidToml(detail) => write!(f, "not valid TOML: {detail}"),
            ConfigError::Validation(errors) => {
                write!(f, "failed validation with {} error(s)", errors.len())
            }
            ConfigError::Exception(name) => write!(f, "raised {name}"),
        }
    }
}

impl std::error::Error for ConfigError {}
