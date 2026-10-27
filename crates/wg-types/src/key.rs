//! Task keys: `<PREFIX>-<n>`, the human-facing name of a task (`WG-12`).

use core::fmt;

use crate::WgError;

/// A validated task key. The original spelling is kept, so `WG-007` round-trips unchanged.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskKey(String);

impl TaskKey {
    /// Parses `<PREFIX>-<n>`: the prefix is an uppercase letter followed by up to nine uppercase
    /// letters or digits; `<n>` is one or more ASCII digits.
    pub fn parse(text: &str) -> Result<Self, WgError> {
        let invalid = || WgError::validation(format!("invalid task key {text:?}"));
        let (prefix, number) = text.split_once('-').ok_or_else(invalid)?;
        let prefix_ok = prefix.len() <= 10
            && prefix.starts_with(|c: char| c.is_ascii_uppercase())
            && prefix
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        let number_ok = !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
        if prefix_ok && number_ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(invalid())
        }
    }

    /// The prefix before the dash, such as `WG`.
    pub fn prefix(&self) -> &str {
        self.0.split_once('-').map_or("", |(prefix, _)| prefix)
    }

    /// The digits after the dash, as written.
    pub fn number(&self) -> &str {
        self.0.split_once('-').map_or("", |(_, number)| number)
    }
}

impl fmt::Display for TaskKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
