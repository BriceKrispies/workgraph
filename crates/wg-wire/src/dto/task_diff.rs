//! The output of `GitService.task_diff` (MCP `git_get_task_diff`, `GET /tasks/{ref}/diff`): a plain
//! dict, not a Pydantic model, so there is no OpenAPI component and it is not listed in
//! [`super::implemented`].
//!
//! The diff text is read live from git by the service; this module shapes the parts and applies
//! the `max_bytes` truncation the service applies. The service measures the limit in characters
//! (Python `len` of a `str`), and so does this module.

use wg_json::{Value, dict};
use wg_types::Id;

use crate::convention::{id, text};
use wg_types::pyval::prefix;

/// The keys of the top-level dict, in the order the service builds them.
pub const FIELDS: [&str; 5] = [
    "task_id",
    "task_key",
    "parts",
    "missing_objects",
    "truncated",
];

/// One diff part: the final diff of a repository's change surface, or one commit's diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffPart {
    /// `kind: "final"`, with `range` as `base12..final12`.
    Final {
        repository: String,
        range: String,
        diff: String,
    },
    /// `kind: "commit"`, with the commit's full SHA.
    Commit {
        repository: String,
        sha: String,
        diff: String,
    },
}

/// A base or commit that git does not have, so its diff could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Missing {
    /// A change surface whose `base..final` range is absent.
    Range { repository: String, range: String },
    /// A commit whose object is absent.
    Commit { repository: String, sha: String },
}

/// The task and its diff parts, before truncation.
pub struct TaskDiff<'a> {
    pub task_id: Id,
    pub task_key: &'a str,
    pub parts: &'a [DiffPart],
    pub missing: &'a [Missing],
    /// The total limit across all parts, in characters.
    pub max_bytes: usize,
}

/// The task-diff dict. Parts are cut in order: each takes what remains of the budget, and a part
/// that is cut gains a `truncated: true` key. `truncated` at the top is true when any part was.
pub fn from_parts(diff: &TaskDiff<'_>) -> Value {
    let mut total = 0_usize;
    let mut truncated = false;
    let mut parts = Vec::with_capacity(diff.parts.len());
    for part in diff.parts {
        let remaining = diff.max_bytes.saturating_sub(total);
        let text_len = part_text(part).chars().count();
        let (body, cut) = if text_len > remaining {
            (prefix(part_text(part), remaining), true)
        } else {
            (part_text(part).to_string(), false)
        };
        total += body.chars().count();
        truncated |= cut;
        parts.push(part_value(part, &body, cut));
    }
    dict!({
        "task_id": id(diff.task_id),
        "task_key": text(diff.task_key),
        "parts": Value::List(parts),
        "missing_objects": Value::List(diff.missing.iter().map(missing_value).collect()),
        "truncated": Value::Bool(truncated),
    })
}

fn part_text(part: &DiffPart) -> &str {
    match part {
        DiffPart::Final { diff, .. } | DiffPart::Commit { diff, .. } => diff,
    }
}

fn part_value(part: &DiffPart, body: &str, cut: bool) -> Value {
    let mut fields = match part {
        DiffPart::Final {
            repository, range, ..
        } => vec![
            ("repository", text(repository)),
            ("kind", text("final")),
            ("range", text(range)),
        ],
        DiffPart::Commit {
            repository, sha, ..
        } => vec![
            ("repository", text(repository)),
            ("kind", text("commit")),
            ("sha", text(sha)),
        ],
    };
    fields.push(("diff", text(body)));
    if cut {
        fields.push(("truncated", Value::Bool(true)));
    }
    dict(fields)
}

fn missing_value(missing: &Missing) -> Value {
    match missing {
        Missing::Range { repository, range } => dict!({
            "repository": text(repository),
            "range": text(range),
        }),
        Missing::Commit { repository, sha } => {
            dict!({ "repository": text(repository), "sha": text(sha) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_json::PyStr;

    #[test]
    fn cuts_by_characters_and_flags_the_cut_part() {
        let parts = [
            DiffPart::Commit {
                repository: "r".into(),
                sha: "abc".into(),
                diff: "héllo".into(),
            },
            DiffPart::Commit {
                repository: "r".into(),
                sha: "def".into(),
                diff: "world".into(),
            },
        ];
        let value = from_parts(&TaskDiff {
            task_id: Id::nil(),
            task_key: "WG-1",
            parts: &parts,
            missing: &[],
            max_bytes: 7,
        });
        let Value::Dict(fields) = value else {
            panic!("diff is a dict");
        };
        let truncated = fields.iter().find(|(k, _)| *k == PyStr::from("truncated"));
        assert!(matches!(truncated, Some((_, Value::Bool(true)))));
    }
}
