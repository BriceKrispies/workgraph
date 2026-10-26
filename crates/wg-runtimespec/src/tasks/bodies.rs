//! The request bodies of the task lifecycle and link routes (`ClaimRequest`, `ReasonBody`,
//! `CompleteBody`, `CommentCreate`, `ProgressReport`, `ResultReport`, `DependencyBody`,
//! `RelationCreate`, `RepoBody`) and the typed values read from a validated body.
//!
//! The models come from the OpenAPI snapshot; `InputSchema` models strip whitespace, the plain
//! pydantic ones do not. A route whose body is optional gives `null` for an absent body, which
//! reads as the model with every default.

use serde_json::{Map, Value};
use wg_types::{CommentKind, ErrorCode, RelationType, WgError};

use super::requests::{component_with, object_of, run, string_of};

/// The model `name` validated against `body`; an absent (`null`) body is the empty object.
fn validated(
    name: &str,
    order: &[&str],
    strip: bool,
    body: &Value,
) -> Result<Map<String, Value>, WgError> {
    let given = if body.is_null() {
        Value::Object(Map::new())
    } else {
        body.clone()
    };
    run(&component_with(name, order, strip)?, &given)
}

fn bad_value(name: &str, value: &str) -> WgError {
    WgError::new(
        ErrorCode::ValidationFailed,
        format!("invalid {name}: {value}"),
    )
}

/// A validated `ClaimRequest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRead {
    pub agent: Option<String>,
    pub lease_minutes: i64,
    pub expected_version: Option<i64>,
}

/// Validates and reads a `ClaimRequest` body.
pub fn read_claim(body: &Value) -> Result<ClaimRead, WgError> {
    let dump = validated(
        "ClaimRequest",
        &["agent", "lease_minutes", "expected_version"],
        true,
        body,
    )?;
    Ok(ClaimRead {
        agent: string_of(&dump, "agent"),
        lease_minutes: dump
            .get("lease_minutes")
            .and_then(Value::as_i64)
            .unwrap_or(120),
        expected_version: dump.get("expected_version").and_then(Value::as_i64),
    })
}

/// Validates a `ReasonBody` (`null` is no body) and reads its reason: `None` for no body.
pub fn read_reason(body: &Value) -> Result<Option<String>, WgError> {
    if body.is_null() {
        return Ok(None);
    }
    let dump = validated("ReasonBody", &["reason"], false, body)?;
    Ok(Some(string_of(&dump, "reason").unwrap_or_default()))
}

/// A validated `CompleteBody`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteRead {
    pub summary: String,
    pub attempt_id: Option<String>,
}

/// Validates and reads a `CompleteBody` (`null` is no body).
pub fn read_complete(body: &Value) -> Result<CompleteRead, WgError> {
    let dump = validated("CompleteBody", &["summary", "attempt_id"], false, body)?;
    Ok(CompleteRead {
        summary: string_of(&dump, "summary").unwrap_or_default(),
        attempt_id: string_of(&dump, "attempt_id"),
    })
}

/// A validated `CommentCreate`.
#[derive(Debug, Clone, PartialEq)]
pub struct CommentRead {
    pub body: String,
    pub kind: CommentKind,
    pub data: Map<String, Value>,
    pub attempt_id: Option<String>,
}

/// Validates and reads a `CommentCreate` body.
pub fn read_comment(body: &Value) -> Result<CommentRead, WgError> {
    let dump = validated(
        "CommentCreate",
        &["body", "kind", "data", "attempt_id"],
        true,
        body,
    )?;
    let kind = string_of(&dump, "kind").unwrap_or_else(|| "comment".to_owned());
    Ok(CommentRead {
        body: string_of(&dump, "body").unwrap_or_default(),
        kind: CommentKind::parse(&kind).ok_or_else(|| bad_value("kind", &kind))?,
        data: object_of(&dump, "data").unwrap_or_default(),
        attempt_id: string_of(&dump, "attempt_id"),
    })
}

/// A validated `ProgressReport`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressRead {
    pub message: String,
    pub percent: Option<f64>,
    pub attempt_id: Option<String>,
    pub data: Map<String, Value>,
}

/// Validates and reads a `ProgressReport` body.
pub fn read_progress(body: &Value) -> Result<ProgressRead, WgError> {
    let dump = validated(
        "ProgressReport",
        &["message", "percent", "attempt_id", "data"],
        true,
        body,
    )?;
    Ok(ProgressRead {
        message: string_of(&dump, "message").unwrap_or_default(),
        percent: dump.get("percent").and_then(Value::as_f64),
        attempt_id: string_of(&dump, "attempt_id"),
        data: object_of(&dump, "data").unwrap_or_default(),
    })
}

/// A validated `ResultReport`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultRead {
    pub attempt_id: Option<String>,
    pub succeeded: bool,
    pub summary: String,
    pub usage: Option<Map<String, Value>>,
    pub cost_usd: Option<f64>,
}

/// Validates and reads a `ResultReport` body.
pub fn read_result(body: &Value) -> Result<ResultRead, WgError> {
    let dump = validated(
        "ResultReport",
        &[
            "attempt_id",
            "outcome",
            "summary",
            "artifacts",
            "usage",
            "cost_usd",
        ],
        true,
        body,
    )?;
    Ok(ResultRead {
        attempt_id: string_of(&dump, "attempt_id"),
        succeeded: string_of(&dump, "outcome").as_deref() == Some("succeeded"),
        summary: string_of(&dump, "summary").unwrap_or_default(),
        usage: object_of(&dump, "usage"),
        cost_usd: dump.get("cost_usd").and_then(Value::as_f64),
    })
}

/// Validates a `DependencyBody` and reads `depends_on`.
pub fn read_dependency(body: &Value) -> Result<String, WgError> {
    let dump = validated("DependencyBody", &["depends_on"], false, body)?;
    Ok(string_of(&dump, "depends_on").unwrap_or_default())
}

/// A validated `RelationCreate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRead {
    pub target: String,
    pub relation_type: RelationType,
}

/// Validates and reads a `RelationCreate` body.
pub fn read_relation(body: &Value) -> Result<RelationRead, WgError> {
    let dump = validated("RelationCreate", &["target", "relation_type"], true, body)?;
    let kind = string_of(&dump, "relation_type").unwrap_or_else(|| "relates_to".to_owned());
    Ok(RelationRead {
        target: string_of(&dump, "target").unwrap_or_default(),
        relation_type: RelationType::parse(&kind)
            .ok_or_else(|| bad_value("relation_type", &kind))?,
    })
}

/// A validated `RepoBody`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRead {
    pub repository: String,
    pub access: Option<String>,
}

/// Validates and reads a `RepoBody`.
pub fn read_repo(body: &Value) -> Result<RepoRead, WgError> {
    let dump = validated("RepoBody", &["repository", "access"], false, body)?;
    Ok(RepoRead {
        repository: string_of(&dump, "repository").unwrap_or_default(),
        access: string_of(&dump, "access"),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_absent_claim_body_is_the_defaults() {
        let read = read_claim(&Value::Null).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            read,
            ClaimRead {
                agent: None,
                lease_minutes: 120,
                expected_version: None
            }
        );
        assert!(read_claim(&json!({"lease_minutes": 0})).is_err());
        assert!(read_claim(&json!({"bogus": 1})).is_err());
    }

    #[test]
    fn a_release_reason_tells_no_body_from_an_empty_one() {
        assert_eq!(read_reason(&Value::Null).ok(), Some(None));
        assert_eq!(read_reason(&json!({})).ok(), Some(Some(String::new())));
        assert_eq!(
            read_reason(&json!({"reason": " x "})).ok(),
            Some(Some(" x ".to_owned()))
        );
    }

    #[test]
    fn input_models_strip_whitespace_and_plain_ones_do_not() {
        let comment = read_comment(&json!({"body": "  hi  "})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(comment.body, "hi");
        assert_eq!(comment.kind, CommentKind::Comment);
        assert!(read_comment(&json!({"body": "   "})).is_err());
        assert_eq!(
            read_dependency(&json!({"depends_on": " WG-1 "})).ok(),
            Some(" WG-1 ".to_owned())
        );
    }

    #[test]
    fn a_result_names_its_outcome() {
        let read = read_result(&json!({"outcome": "failed", "cost_usd": 0.5}))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(!read.succeeded);
        assert_eq!(read.cost_usd, Some(0.5));
        assert!(read_result(&json!({"outcome": "maybe"})).is_err());
    }

    #[test]
    fn a_relation_defaults_to_relates_to() {
        let read = read_relation(&json!({"target": "WG-2"})).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(read.relation_type, RelationType::RelatesTo);
    }
}
