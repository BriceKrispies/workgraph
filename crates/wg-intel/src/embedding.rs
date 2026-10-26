//! The pure parts of the embedding pipeline and of similar-task search (`search/service.py`): the
//! text that is embedded, its content digest, the packed vector format, and the score of a similar
//! task.
//!
//! The provider calls, the stored rows and the ranking of the workspace stay in the shell.

use std::collections::BTreeSet;

use wg_types::Id;

/// The longest text sent to the embedding provider (`[:8000]`, in characters).
const EMBEDDING_TEXT_CHARS: usize = 8000;

/// The weight of a shared category in a similar task's score.
pub const SAME_CATEGORY_WEIGHT: f64 = 0.1;

/// The weight of the repository overlap in a similar task's score.
pub const REPOSITORY_WEIGHT: f64 = 0.1;

/// `embedding_text(task)`: the title, the description, the acceptance criteria and the tags,
/// joined by newlines with the empty parts dropped, cut to 8000 characters. The tags are one
/// space-joined part.
#[must_use]
pub fn embedding_text<'a>(
    title: &str,
    description: &str,
    criteria: impl IntoIterator<Item = &'a str>,
    tags: impl IntoIterator<Item = &'a str>,
) -> String {
    let tag_part = tags.into_iter().collect::<Vec<&str>>().join(" ");
    [title, description]
        .into_iter()
        .map(str::to_owned)
        .chain(criteria.into_iter().map(str::to_owned))
        .chain(std::iter::once(tag_part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<String>>()
        .join("\n")
        .chars()
        .take(EMBEDDING_TEXT_CHARS)
        .collect()
}

/// The `input_hash` of an embedded text: its SHA-256 in hex.
#[must_use]
pub fn content_digest(text: &str) -> String {
    wg_hash::sha256_hex(text.as_bytes())
}

/// `pack(vector)`: the vector as little-endian 32-bit floats, the `array('f')` layout.
#[must_use]
pub fn pack(vector: &[f64]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| (*value as f32).to_le_bytes())
        .collect()
}

/// `unpack(blob)`: the 32-bit floats of a packed vector. A trailing partial value is dropped.
#[must_use]
pub fn unpack(blob: &[u8]) -> Vec<f64> {
    blob.chunks_exact(4)
        .map(|chunk| f64::from(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])))
        .collect()
}

/// The repository overlap of two tasks: the shared repositories over all of theirs, 0 when
/// neither has one.
#[must_use]
pub fn repository_overlap(mine: &BTreeSet<Id>, theirs: &BTreeSet<Id>) -> f64 {
    let shared = mine.intersection(theirs).count();
    let all = mine.union(theirs).count();
    if all == 0 {
        0.0
    } else {
        shared as f64 / all as f64
    }
}

/// The score of a similar task: its similarity, plus the category bonus when the categories are
/// set and equal, plus the repository overlap bonus.
#[must_use]
pub fn similar_score(similarity: f64, same_category: bool, overlap: f64) -> f64 {
    similarity
        + if same_category {
            SAME_CATEGORY_WEIGHT
        } else {
            0.0
        }
        + REPOSITORY_WEIGHT * overlap
}

/// Whether two optional categories are set and equal (`category and other.category == category`).
#[must_use]
pub fn same_category(mine: Option<&str>, theirs: Option<&str>) -> bool {
    matches!((mine, theirs), (Some(a), Some(b)) if !a.is_empty() && a == b)
}
