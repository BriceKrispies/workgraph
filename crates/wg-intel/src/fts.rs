//! FTS5 `MATCH` expressions built from free text (`workgraph.search.indexer.to_fts_query`).
//!
//! Every token is a run of word characters, quoted, so no operator in the user's text reaches the
//! FTS parser.

use std::sync::LazyLock;

use regex::Regex;

use crate::order::unique;

/// Matching options. `prefix` puts a wildcard on the last token (search as you type); `match_any`
/// joins the tokens with `OR` instead of AND, and keeps the first 40 of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FtsOptions {
    pub prefix: bool,
    pub match_any: bool,
}

impl Default for FtsOptions {
    fn default() -> Self {
        Self {
            prefix: true,
            match_any: false,
        }
    }
}

/// Tokens counted by `match_any`.
const MATCH_ANY_TOKENS: usize = 40;

static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\w+").unwrap_or_else(|error| panic!("built-in token pattern: {error}"))
});

/// The FTS5 expression for `user_query`, or `None` when it has no word characters.
#[must_use]
pub fn to_fts_query(user_query: &str, options: FtsOptions) -> Option<String> {
    let tokens: Vec<&str> = TOKEN.find_iter(user_query).map(|m| m.as_str()).collect();
    let last = tokens.len().checked_sub(1)?;
    let quoted: Vec<String> = tokens
        .iter()
        .enumerate()
        .map(|(index, token)| {
            if options.prefix && index == last {
                format!("\"{token}\"*")
            } else {
                format!("\"{token}\"")
            }
        })
        .collect();
    if options.match_any {
        Some(unique(quoted.into_iter().take(MATCH_ANY_TOKENS)).join(" OR "))
    } else {
        Some(quoted.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_quoted_and_the_last_one_is_a_prefix() {
        assert_eq!(
            to_fts_query("token refresh", FtsOptions::default()).as_deref(),
            Some("\"token\" \"refresh\"*")
        );
        assert_eq!(to_fts_query("!!!", FtsOptions::default()), None);
    }
}
