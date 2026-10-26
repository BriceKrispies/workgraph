//! LIB-05 (registry part): the workspace and project documents list their fields in order, and a
//! workspace prints as the model dump does. The DTOs moved with the registry feature.

#![allow(clippy::expect_used)]

use serde_json::{Map, json};
use wg_json::{Value, http_json};
use wg_model::entity::project::Project;
use wg_model::entity::workspace::Workspace;
use wg_registry_feature::spec::dto::{project, workspace};
use wg_types::{Id, Timestamp, parse_id};

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
