//! The content hash of a task's title and description (`content_hash` in
//! `core/services/tasks.py`). A change to the hash marks a material edit, which queues
//! reclassification and re-embedding.
//!
//! The digest comes from `wg-hash`, the shared in-crate SHA-256.

use wg_hash::sha256_hex;

/// `content_hash(title, description)`: the SHA-256 of the whitespace-normalised, lowercased title
/// and description, joined by a newline. Whitespace runs collapse to one space and the ends are
/// trimmed, so an edit that only reflows text leaves the hash unchanged.
pub fn content_hash(title: &str, description: &str) -> String {
    let normalised = format!("{}\n{}", normalise(title), normalise(description));
    sha256_hex(normalised.as_bytes())
}

fn normalise(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_ignores_case_and_whitespace_layout() {
        assert_eq!(
            content_hash("  Fix   the Build ", "Line one\n\n  two "),
            content_hash("fix the build", "line one two")
        );
        assert_ne!(content_hash("Fix", "a"), content_hash("Fix", "b"));
    }
}
