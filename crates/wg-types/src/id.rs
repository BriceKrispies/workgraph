//! Entity ids: UUID strings in canonical lowercase hyphenated form.

use uuid::Uuid;

use crate::WgError;

/// An entity id. Generation (UUIDv4) happens in the shell; Core only carries and compares them.
pub type Id = Uuid;

/// Parses a UUID string. Any case is accepted; [`Id`] displays the canonical form (lowercase,
/// hyphenated).
pub fn parse_id(text: &str) -> Result<Id, WgError> {
    Uuid::parse_str(text).map_err(|err| WgError::validation(format!("invalid id {text:?}: {err}")))
}

/// A request's correlation id, as the events carry it. Python stores it as text: a fresh UUID, or
/// the text a client sent in `X-Correlation-Id` (any text up to 64 characters), so it is text here.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Correlation(String);

impl Correlation {
    /// A correlation with the given text, kept as it is.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text of the correlation id, as the events table and the API show it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Id> for Correlation {
    fn from(id: Id) -> Self {
        Self(id.to_string())
    }
}

impl std::fmt::Display for Correlation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
