//! Task comments, progress reports and result comments (CORE-04). Mirrors `_comment`,
//! `add_comment`, `report_progress` and the attempt-ownership check of `_comment` in
//! `core/services/tasks.py`.
//!
//! The shell stores the comment row, writes the event named by [`event_type`], and indexes the
//! body for search. These functions build the payloads and run the checks.

use serde_json::{Map, Value, json};
use wg_types::pyval::prefix;
use wg_types::{CommentKind, Id, WgError};

/// Characters of the body kept in the event payload's `excerpt`.
pub const EXCERPT_CHARS: usize = 500;

/// The event type a comment of `kind` writes.
pub const fn event_type(kind: CommentKind) -> &'static str {
    match kind {
        CommentKind::Comment => "task.comment_added",
        CommentKind::Progress => "task.progress_reported",
        CommentKind::Result => "task.result_reported",
    }
}

/// The first [`EXCERPT_CHARS`] characters of a body.
pub fn excerpt(body: &str) -> String {
    prefix(body, EXCERPT_CHARS)
}

/// A comment given an attempt must name an attempt of the same task.
pub fn check_attempt_owner(task_id: Id, attempt_task_id: Id) -> Result<(), WgError> {
    if attempt_task_id == task_id {
        Ok(())
    } else {
        Err(WgError::validation("attempt does not belong to this task"))
    }
}

/// The payload of the comment's event: `comment_id`, `kind` and `excerpt`, then the caller's
/// `data` merged over them (Python's `{..., **data.data}`).
pub fn comment_event_data(
    comment_id: Id,
    kind: CommentKind,
    body: &str,
    data: &Map<String, Value>,
) -> Value {
    Value::Object(
        [
            ("comment_id".to_owned(), json!(comment_id.to_string())),
            ("kind".to_owned(), json!(kind.as_str())),
            ("excerpt".to_owned(), json!(excerpt(body))),
        ]
        .into_iter()
        .chain(data.iter().map(|(key, value)| (key.clone(), value.clone())))
        .collect(),
    )
}

/// The `data` stored with a progress comment: the caller's data, plus `percent` when given
/// (`report_progress`).
pub fn progress_data(data: &Map<String, Value>, percent: Option<f64>) -> Map<String, Value> {
    data.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .chain(percent.map(|percent| ("percent".to_owned(), json!(percent))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_follow_the_comment_kind() {
        assert_eq!(event_type(CommentKind::Comment), "task.comment_added");
        assert_eq!(event_type(CommentKind::Progress), "task.progress_reported");
        assert_eq!(event_type(CommentKind::Result), "task.result_reported");
    }

    #[test]
    fn excerpt_keeps_the_first_500_characters() {
        let body = "é".repeat(600);
        assert_eq!(excerpt(&body).chars().count(), 500);
        assert_eq!(excerpt("short"), "short");
    }
}
