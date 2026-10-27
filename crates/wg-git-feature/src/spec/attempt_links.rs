//! The pure half of `GitService.link_attempt_commits`: the repositories an attempt changed, read
//! from its baseline and outcome manifests, and the evidence its links carry.

use rpds::VectorSync;
use serde_json::{Value as Json, json};
use wg_model::entity::JsonObject;
use wg_types::{Id, parse_id};

/// A repository an attempt changed: its id, the baseline and outcome shas and the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub repository_id: Id,
    pub base: String,
    pub head: String,
    pub path: String,
}

fn text(entry: &JsonObject, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(Json::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The first twelve characters of a sha (`sha[:12]`).
fn short(sha: &str) -> String {
    sha.chars().take(12).collect()
}

/// The repositories whose outcome sha differs from their baseline sha, in outcome order. A
/// repository missing a sha on either side, or a checkout path, is left out.
#[must_use]
pub fn ranges(baseline: &VectorSync<JsonObject>, outcome: &VectorSync<JsonObject>) -> Vec<Range> {
    outcome
        .iter()
        .filter_map(|entry| {
            let repository_id = parse_id(&text(entry, "repository_id")?).ok()?;
            let base = baseline
                .iter()
                .find(|other| text(other, "repository_id") == text(entry, "repository_id"))
                .and_then(|other| text(other, "sha"))?;
            Some(Range {
                repository_id,
                base,
                head: text(entry, "sha")?,
                path: text(entry, "checkout_path")?,
            })
        })
        .filter(|range| range.base != range.head)
        .collect()
}

/// The evidence of an execution-context link: the attempt and the abbreviated range.
#[must_use]
pub fn evidence(attempt_id: Id, range: &Range) -> JsonObject {
    [
        ("attempt_id", json!(attempt_id.to_string())),
        (
            "range",
            json!(format!("{}..{}", short(&range.base), short(&range.head))),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}
