//! Entity ids: UUID strings in canonical lowercase hyphenated form.

use uuid::Uuid;

use crate::{ErrorCode, WgError};

/// An entity id. Generation (UUIDv4) happens in the shell; Core only carries and compares them.
pub type Id = Uuid;

/// Parses a UUID string. Any case is accepted; [`Id`] displays the canonical form (lowercase,
/// hyphenated).
pub fn parse_id(text: &str) -> Result<Id, WgError> {
    Uuid::parse_str(text).map_err(|err| {
        WgError::new(
            ErrorCode::ValidationFailed,
            format!("invalid id {text:?}: {err}"),
        )
    })
}
