//! The request bodies of the task tools (M26 to M46, `src/workgraph/mcp/server.py`).
//!
//! Each tool builds the same pydantic model its Python method takes (`TaskCreate`, `TaskFilter`,
//! `TaskUpdate`, `ClaimRequest`, ...); here the validated arguments become the JSON body the
//! feature's services read, with the fields the Python tool passes. An argument the call left out
//! is `null` after validation, and a `null` is an absent field, as Python's `is not None` checks
//! read it.

use serde_json::{Map, Value, json};
use wg_types::WgError;
use wg_types::pyval;

/// The argument `key` when the call gave it (a `null` is absent).
fn given(arguments: &Value, key: &str) -> Option<Value> {
    arguments.get(key).filter(|value| !value.is_null()).cloned()
}

/// The string argument `key`, or "" when absent.
pub fn text(arguments: &Value, key: &str) -> String {
    pyval::text(arguments.get(key)).to_owned()
}

/// The boolean argument `key`, false when absent.
fn flag(arguments: &Value, key: &str) -> bool {
    pyval::flag(arguments.get(key))
}

/// The integer argument `key`, or `default` when absent.
pub fn number(arguments: &Value, key: &str, default: i64) -> i64 {
    pyval::int_or(arguments.get(key), default)
}

/// The list argument `key` (`or []` in Python), or an empty list.
fn list(arguments: &Value, key: &str) -> Value {
    given(arguments, key).unwrap_or_else(|| json!([]))
}

/// An object of the arguments named in `keys` that the call gave, as the model's fields.
fn picked(arguments: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| given(arguments, key).map(|value| ((*key).to_owned(), value)))
            .collect::<Map<String, Value>>(),
    )
}

/// The `TaskCreate` body of `task_create`: `ready` chooses the status, and the lists default to
/// empty (`acceptance_criteria or []` and so on).
pub fn create_body(arguments: &Value) -> Value {
    json!({
        "title": text(arguments, "title"),
        "description": text(arguments, "description"),
        "acceptance_criteria": list(arguments, "acceptance_criteria"),
        "priority": given(arguments, "priority").unwrap_or_else(|| json!("medium")),
        "status": if flag(arguments, "ready") { "ready" } else { "backlog" },
        "tags": list(arguments, "tags"),
        "files": list(arguments, "files"),
        "projects": list(arguments, "projects"),
        "repositories": list(arguments, "repositories"),
        "parent": given(arguments, "parent"),
        "depends_on": list(arguments, "depends_on"),
    })
}

/// The `TaskFilter` body of `task_list`: the filter, sort and page the call gave.
pub fn filter_body(arguments: &Value) -> Value {
    picked(
        arguments,
        &[
            "statuses",
            "priorities",
            "project",
            "repository",
            "tags",
            "file",
            "category",
            "parent",
            "text",
            "sort",
            "descending",
            "limit",
            "offset",
        ],
    )
}

/// The `TaskUpdate` body of `task_update`: the fields the call gave, `files: []` included.
pub fn update_body(arguments: &Value) -> Value {
    picked(
        arguments,
        &[
            "expected_version",
            "title",
            "description",
            "status",
            "priority",
            "add_tags",
            "remove_tags",
            "acceptance_criteria",
            "files",
            "add_files",
            "reason",
        ],
    )
}

/// The `ClaimRequest` body of `task_claim`.
pub fn claim_body(arguments: &Value) -> Value {
    json!({ "lease_minutes": number(arguments, "lease_minutes", 120) })
}

/// The optional `ReasonBody` of `task_release`: no body without a reason.
pub fn release_body(arguments: &Value) -> Value {
    match given(arguments, "reason") {
        None => Value::Null,
        Some(reason) => json!({ "reason": reason }),
    }
}

/// The `CompleteBody` of `task_complete`.
pub fn complete_body(arguments: &Value) -> Value {
    json!({
        "summary": text(arguments, "summary"),
        "attempt_id": given(arguments, "attempt_id"),
    })
}

/// The `ResultReport` of `task_report_result`. `usage` is given only when the runtime measured
/// some of it (`usage or None` in Python).
pub fn result_body(arguments: &Value) -> Value {
    let usage = picked(arguments, &["input_tokens", "output_tokens"]);
    let usage = match usage.as_object() {
        Some(map) if !map.is_empty() => usage,
        _ => Value::Null,
    };
    json!({
        "outcome": text(arguments, "outcome"),
        "summary": text(arguments, "summary"),
        "attempt_id": given(arguments, "attempt_id"),
        "usage": usage,
    })
}

/// The `ProgressReport` body of `task_report_progress`.
pub fn progress_body(arguments: &Value) -> Value {
    json!({
        "message": text(arguments, "message"),
        "percent": given(arguments, "percent"),
        "attempt_id": given(arguments, "attempt_id"),
    })
}

/// The `CommentCreate` body of `task_add_comment`.
pub fn comment_body(arguments: &Value) -> Value {
    json!({ "body": text(arguments, "body") })
}

/// The `DependencyBody` of `task_add_dependency`.
pub fn dependency_body(arguments: &Value) -> Value {
    json!({ "depends_on": text(arguments, "depends_on") })
}

/// The `RelationCreate` body of `task_add_relation` (`relates_to` unless the call names a type).
pub fn relation_body(arguments: &Value) -> Value {
    json!({
        "target": text(arguments, "target"),
        "relation_type": given(arguments, "relation_type").unwrap_or_else(|| json!("relates_to")),
    })
}

/// The `RepoBody` of `task_link_repository`: `read_only` is the access, else none.
pub fn repo_body(arguments: &Value) -> Value {
    json!({
        "repository": text(arguments, "repository"),
        "access": if flag(arguments, "read_only") { Some("read_only") } else { None },
    })
}

/// The scope `task_promote_ready` gives, checked as Python checks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromoteScope {
    pub tags: Vec<String>,
    pub project: Option<String>,
    pub dry_run: bool,
}

/// The promote scope of the call: `all` cannot be combined with tags or a project, and a call
/// needs one of the three (`all`, `tags` or `project`). `all` alone promotes every backlog task.
pub fn promote_scope(arguments: &Value) -> Result<PromoteScope, WgError> {
    let tags: Vec<String> = arguments
        .get("tags")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let project = arguments
        .get("project")
        .and_then(Value::as_str)
        .filter(|project| !project.is_empty())
        .map(str::to_owned);
    let all = flag(arguments, "all");
    let scoped = project.is_some() || !tags.is_empty();
    if all && scoped {
        return Err(WgError::validation(
            "all=true cannot be combined with tags or project",
        ));
    }
    if !(all || scoped) {
        return Err(WgError::validation(
            "give tags, project, or all=true to choose the tasks to promote",
        ));
    }
    Ok(PromoteScope {
        tags,
        project,
        dry_run: flag(arguments, "dry_run"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_defaults_to_backlog_and_empty_lists() {
        let body = create_body(
            &json!({"title": "T", "description": "", "priority": "medium",
            "ready": false, "acceptance_criteria": null, "tags": null}),
        );
        assert_eq!(body["status"], json!("backlog"));
        assert_eq!(body["tags"], json!([]));
        assert_eq!(body["parent"], Value::Null);
    }

    #[test]
    fn update_keeps_only_given_fields() {
        let body = update_body(&json!({"task": "WG-1", "title": "x", "files": [], "status": null}));
        assert_eq!(body, json!({"title": "x", "files": []}));
    }

    #[test]
    fn result_usage_only_when_measured() {
        let none =
            result_body(&json!({"outcome": "failed", "input_tokens": null, "output_tokens": null}));
        assert_eq!(none["usage"], Value::Null);
        let some =
            result_body(&json!({"outcome": "succeeded", "input_tokens": 3, "output_tokens": null}));
        assert_eq!(some["usage"], json!({"input_tokens": 3}));
    }

    #[test]
    fn promote_scope_rules_match_python() {
        let all = promote_scope(&json!({"all": true, "tags": ["a"]}));
        assert!(all.is_err());
        assert!(promote_scope(&json!({"all": false, "tags": [], "project": null})).is_err());
        let by_tag = promote_scope(&json!({"all": false, "tags": ["a"], "project": null}));
        assert_eq!(by_tag.map(|scope| scope.tags), Ok(vec!["a".to_owned()]));
        assert!(promote_scope(&json!({"all": true, "tags": [], "project": null})).is_ok());
    }
}
