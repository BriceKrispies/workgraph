//! The output of `GitService.task_diff` (MCP `git_get_task_diff`, `GET /tasks/{ref}/diff`): a plain
//! dict, not a Pydantic model, so there is no OpenAPI component and it is not listed in
//! [`super::implemented`].
//!
//! The diff text is read live from git by the service; this module shapes the parts and applies
//! the `max_bytes` truncation the service applies. The service measures the limit in characters
//! (Python `len` of a `str`), and so does this module.

use serde::Serialize;
use wg_json::Value;
use wg_types::Id;
use wg_types::pyval::prefix;

use crate::dto::Dto;
use crate::wire_dto;

wire_dto! {
    /// The task-diff dict, keys in the order the service builds them.
    pub struct TaskDiffOut<'a>("TaskDiffOut") {
        pub task_id: Id,
        pub task_key: &'a str,
        pub parts: Vec<PartOut<'a>>,
        pub missing_objects: Vec<MissingOut<'a>>,
        pub truncated: bool,
    }
}

/// One part of the output: `repository`, `kind`, then `range` (final) or `sha` (commit), the
/// `diff` text, and `truncated: true` on a part that was cut.
#[derive(Debug, Serialize)]
pub struct PartOut<'a> {
    pub repository: &'a str,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<&'a str>,
    pub diff: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// One entry of `missing_objects`: the repository and the absent `range` or `sha`.
#[derive(Debug, Serialize)]
pub struct MissingOut<'a> {
    pub repository: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<&'a str>,
}

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
    // Each part takes what remains of the budget: (body, cut) per part, in order.
    let bodies: Vec<(String, bool)> = diff
        .parts
        .iter()
        .scan(0_usize, |total, part| {
            let remaining = diff.max_bytes.saturating_sub(*total);
            let cut = part_text(part).chars().count() > remaining;
            let body = if cut {
                prefix(part_text(part), remaining)
            } else {
                part_text(part).to_string()
            };
            *total += body.chars().count();
            Some((body, cut))
        })
        .collect();
    TaskDiffOut {
        task_id: diff.task_id,
        task_key: diff.task_key,
        truncated: bodies.iter().any(|(_, cut)| *cut),
        parts: diff
            .parts
            .iter()
            .zip(&bodies)
            .map(|(part, (body, cut))| part_out(part, body, *cut))
            .collect(),
        missing_objects: diff.missing.iter().map(missing_out).collect(),
    }
    .dict()
}

fn part_text(part: &DiffPart) -> &str {
    match part {
        DiffPart::Final { diff, .. } | DiffPart::Commit { diff, .. } => diff,
    }
}

fn part_out<'a>(part: &'a DiffPart, body: &'a str, cut: bool) -> PartOut<'a> {
    let (repository, kind, range, sha) = match part {
        DiffPart::Final {
            repository, range, ..
        } => (repository, "final", Some(range.as_str()), None),
        DiffPart::Commit {
            repository, sha, ..
        } => (repository, "commit", None, Some(sha.as_str())),
    };
    PartOut {
        repository,
        kind,
        range,
        sha,
        diff: body,
        truncated: cut.then_some(true),
    }
}

fn missing_out(missing: &Missing) -> MissingOut<'_> {
    match missing {
        Missing::Range { repository, range } => MissingOut {
            repository,
            range: Some(range),
            sha: None,
        },
        Missing::Commit { repository, sha } => MissingOut {
            repository,
            range: None,
            sha: Some(sha),
        },
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
