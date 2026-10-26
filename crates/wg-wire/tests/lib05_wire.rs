//! LIB-05: the error envelope, the DTO field orders against the OpenAPI component schemas, and the
//! embedded descriptors.
//!
//! The component-schema test checks only the DTOs listed by `wg_wire::dto::implemented` (plan
//! 5.5); the other schemas are reported on stderr, never failed.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value as Json, json};
use wg_json::{Value, cli_json, http_json};
use wg_model::entity::project::Project;
use wg_model::entity::workspace::Workspace;
use wg_types::{ErrorCode, Id, Timestamp, WgError, parse_id};
use wg_wire::contract::{component_schemas, mcp_tools};
use wg_wire::dto::{self, project, workspace};
use wg_wire::error::{cli_error_envelope, error_envelope, validation_envelope};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Field names of every `class X(Schema)` of the Python source, in declaration order.
fn python_fields() -> Option<Vec<(String, Vec<String>)>> {
    let text = fs::read_to_string(root().join("src/workgraph/core/models/schemas.py")).ok()?;
    let classes =
        text.lines().fold(
            Vec::<(String, Vec<String>)>::new(),
            |classes, line| match class_name(line) {
                Some(name) => classes
                    .into_iter()
                    .chain(std::iter::once((name, Vec::new())))
                    .collect(),
                None => match (field_name(line), classes.split_last()) {
                    (Some(field), Some((last, rest))) => rest
                        .iter()
                        .cloned()
                        .chain(std::iter::once((
                            last.0.clone(),
                            last.1.iter().cloned().chain([field]).collect(),
                        )))
                        .collect(),
                    _ => classes,
                },
            },
        );
    Some(classes)
}

fn class_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("class ")?;
    let (name, _) = rest.split_once('(')?;
    Some(name.to_string())
}

/// An annotated field of a class body: four spaces, an identifier, a colon.
fn field_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("    ")?;
    let (name, _) = rest.split_once(':')?;
    let valid = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit() || c.is_whitespace())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then(|| name.to_string())
}

fn strings(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn implemented_dtos_match_the_openapi_component_schemas() {
    let schemas = component_schemas().expect("snapshot");
    let python = python_fields();
    for (name, fields) in dto::implemented() {
        let schema = schemas
            .get(name)
            .unwrap_or_else(|| panic!("{name} is not an OpenAPI component schema"));
        let dto_fields: Vec<String> = fields.iter().map(|f| (*f).to_string()).collect();

        let properties: BTreeSet<String> = schema
            .get("properties")
            .and_then(Json::as_object)
            .map(|props| props.keys().cloned().collect())
            .unwrap_or_default();
        assert_eq!(
            dto_fields.iter().cloned().collect::<BTreeSet<_>>(),
            properties,
            "{name}: field names differ from the component schema"
        );

        // `required` keeps declaration order (properties are sorted by name in the snapshot), so
        // the required fields must appear in the DTO in that order.
        let required = strings(schema.get("required"));
        let in_dto_order: Vec<String> = dto_fields
            .iter()
            .filter(|f| required.contains(f))
            .cloned()
            .collect();
        assert_eq!(
            required, in_dto_order,
            "{name}: required field order differs"
        );

        if let Some(python) = &python {
            let class = python
                .iter()
                .find(|(class, _)| class == name)
                .unwrap_or_else(|| panic!("{name} is not a class of schemas.py"));
            assert_eq!(dto_fields, class.1, "{name}: order differs from schemas.py");
        }
    }
}

#[test]
fn unimplemented_schemas_are_reported_not_failed() {
    let schemas = component_schemas().expect("snapshot");
    let done: BTreeSet<&str> = dto::implemented().iter().map(|(name, _)| *name).collect();
    let pending: Vec<&String> = schemas
        .keys()
        .filter(|name| !done.contains(name.as_str()))
        .collect();
    eprintln!(
        "wg-wire: {} of {} component schemas have a DTO; {} pending",
        done.len(),
        schemas.len(),
        pending.len()
    );
    assert!(done.len() <= schemas.len());
}

fn at(micros: u32) -> Timestamp {
    Timestamp::from_components(2026, 10, 25, 13, 56, 0, micros).expect("timestamp")
}

fn id_of(text: &str) -> Id {
    parse_id(text).expect("id")
}

fn names(value: &Value) -> Vec<String> {
    match value {
        Value::Model { fields, .. } => fields.iter().map(|(k, _)| k.clone()).collect(),
        other => panic!("not a model: {other:?}"),
    }
}

fn sample_workspace() -> Workspace {
    Workspace {
        id: id_of("00000000-0000-4000-8000-000000000001"),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 7,
        settings: Map::from_iter([("b".to_string(), json!(1)), ("a".to_string(), json!(null))]),
        archived_at: None,
        created_at: at(0),
        updated_at: at(250_000),
    }
}

#[test]
fn dto_values_list_their_fields_in_order() {
    let ws = workspace::from_model(&sample_workspace());
    assert_eq!(names(&ws), workspace::FIELDS);

    let project_row = Project {
        id: id_of("00000000-0000-4000-8000-000000000002"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        slug: "p".into(),
        name: "P".into(),
        description: "d".into(),
        status: "active".into(),
        settings: Map::new(),
        created_at: at(0),
        updated_at: at(0),
    };
    let proj = project::from_model(
        &project_row,
        [id_of("00000000-0000-4000-8000-000000000003")],
    );
    assert_eq!(names(&proj), project::FIELDS);
}

#[test]
fn workspace_prints_as_model_dump_does() {
    let text = http_json(&workspace::from_model(&sample_workspace())).expect("dump");
    assert_eq!(
        text,
        concat!(
            r#"{"id":"00000000-0000-4000-8000-000000000001","slug":"default","name":"Default","#,
            r#""description":"","key_prefix":"WG","settings":{"b":1,"a":null},"#,
            r#""created_at":"2026-10-25T13:56:00Z","updated_at":"2026-10-25T13:56:00.250000Z","#,
            r#""archived_at":null}"#
        )
    );
}

#[test]
fn error_envelopes() {
    let error = WgError::new(ErrorCode::NotFound, "no task WG-9")
        .with_detail("key", json!("WG-9"))
        .with_detail("kind", json!("task"));
    assert_eq!(error.http_status(), 404);
    assert_eq!(
        http_json(&error_envelope(&error)).expect("dump"),
        r#"{"error":{"code":"not_found","message":"no task WG-9","details":{"key":"WG-9","kind":"task"}}}"#
    );
    assert_eq!(
        cli_json(&cli_error_envelope("bad")).expect("dump"),
        "{\n  \"error\": {\n    \"code\": \"cli_error\",\n    \"message\": \"bad\",\n    \"details\": {}\n  }\n}\n"
    );
    let item =
        json!({"type": "missing", "loc": ["body", "slug"], "msg": "Field required", "input": {}});
    assert_eq!(
        http_json(&validation_envelope(&[item])).expect("dump"),
        concat!(
            r#"{"error":{"code":"validation_failed","message":"invalid request","details":"#,
            r#"{"errors":[{"type":"missing","loc":["body","slug"],"msg":"Field required","input":{}}]}}}"#
        )
    );
}

#[test]
fn embedded_descriptors_parse() {
    let tools = mcp_tools().expect("tools");
    assert!(!tools.is_empty());
    assert!(
        tools
            .iter()
            .all(|tool| tool.get("name").is_some() && tool.get("input_schema").is_some())
    );
    assert!(!component_schemas().expect("schemas").is_empty());
}
