//! The acting identity recorded on events: `<kind>:<name>`, such as `human:brice`.

use core::fmt;

use crate::{ActorKind, WgError};

/// Who performed an action.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Actor {
    kind: ActorKind,
    name: String,
}

impl Actor {
    /// Builds an actor. The name must be non-empty.
    pub fn new(kind: ActorKind, name: &str) -> Result<Self, WgError> {
        if name.is_empty() {
            return Err(WgError::validation("actor name is empty"));
        }
        Ok(Self {
            kind,
            name: name.to_owned(),
        })
    }

    /// Parses `<kind>:<name>`. The name may itself contain colons.
    pub fn parse(text: &str) -> Result<Self, WgError> {
        let (kind, name) = text
            .split_once(':')
            .ok_or_else(|| WgError::validation(format!("invalid actor {text:?}")))?;
        let kind = ActorKind::parse(kind)
            .ok_or_else(|| WgError::validation(format!("unknown actor kind in {text:?}")))?;
        Self::new(kind, name)
    }

    pub fn kind(&self) -> ActorKind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.name)
    }
}
