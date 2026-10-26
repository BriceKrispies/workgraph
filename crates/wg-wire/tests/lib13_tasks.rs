//! LIB-13: the task-area DTOs. Each DTO built from model values lists its fields in the order of
//! its `FIELDS` constant, and the values print as the API prints them.

#![allow(clippy::expect_used)]

use rpds::VectorSync;
use serde_json::{Map, json};
use wg_json::{Value, http_json};
use wg_model::entity::JsonObject;
use wg_model::entity::task::{
    DecisionOption, Task, TaskComment, TaskDecision, TaskFile, TaskLink, TaskRelation, TaskTag,
};
use wg_types::{
    CommentKind, Id, RelationType, TaskKey, TaskPriority, TaskStatus, Timestamp, parse_id,
};
use wg_wire::dto::decision;
use wg_wire::dto::task::TaskRelated;
use wg_wire::dto::{
    comment, decision_choose, decision_option, decision_option_in, decision_request, decision_task,
    decision_with_task, decision_withdraw, dependency_state, graph_edge, graph_node, page,
    relation, task, task_brief, task_detail, task_file, task_graph, task_link, task_rollup,
};

fn id_of(text: &str) -> Id {
    parse_id(text).expect("id")
}

fn at(micros: u32) -> Timestamp {
    Timestamp::from_components(2026, 10, 25, 13, 56, 0, micros).expect("timestamp")
}

fn names(value: &Value) -> Vec<String> {
    match value {
        Value::Model { fields, .. } => fields.iter().map(|(k, _)| k.clone()).collect(),
        other => panic!("not a model: {other:?}"),
    }
}

fn task_row() -> Task {
    Task {
        id: id_of("00000000-0000-4000-8000-000000000010"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        number: 7,
        key: TaskKey::parse("WG-7").expect("key"),
        title: "Wire DTOs".into(),
        description: "d".into(),
        description_hash: "h".into(),
        acceptance_criteria: VectorSync::new_sync().push_back("a".to_string()),
        status: TaskStatus::Ready,
        priority: TaskPriority::High,
        category: None,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        claimed_by: None,
        claim_expires_at: None,
        auto_execute: false,
        estimated_effort: None,
        metadata: JsonObject::new(),
        due_at: None,
        scheduled_at: None,
        created_by: "human:Brice".into(),
        version: 3,
        created_at: at(0),
        updated_at: at(250_000),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

fn file_row() -> TaskFile {
    TaskFile {
        task_id: task_row().id,
        position: 0,
        path: "src/lib.rs".into(),
        anchor: None,
        note: String::new(),
    }
}

fn link_row() -> TaskLink {
    TaskLink {
        id: id_of("00000000-0000-4000-8000-000000000020"),
        task_id: task_row().id,
        kind: "pull_request".into(),
        provider: "github".into(),
        url: "https://github.com/o/r/pull/1".into(),
        normalized_url: "https://github.com/o/r/pull/1".into(),
        repo_full_name: "o/r".into(),
        number: Some(1),
        repository_id: None,
        sources: VectorSync::new_sync().push_back("branch".to_string()),
        title: "PR".into(),
        created_by: "system".into(),
        created_at: at(0),
        updated_at: at(0),
    }
}

fn decision_row() -> TaskDecision {
    TaskDecision {
        id: id_of("00000000-0000-4000-8000-000000000030"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        task_id: task_row().id,
        question: "Which?".into(),
        options: vec![DecisionOption {
            id: "a".into(),
            label: "A".into(),
            detail: String::new(),
            recommended: true,
        }]
        .into_iter()
        .collect(),
        status: "open".into(),
        chosen_option_id: None,
        other_text: None,
        note: None,
        decided_by: None,
        decided_at: None,
        created_by: "agent:claude".into(),
        created_at: at(0),
        updated_at: at(0),
        version: 1,
    }
}

#[test]
fn every_task_dto_lists_its_fields_in_order() {
    let task_row = task_row();
    let files = [file_row()];
    let links = [link_row()];
    let tags = [TaskTag {
        task_id: task_row.id,
        tag: "rust".into(),
    }];
    let related = TaskRelated {
        tags: &tags,
        files: &files,
        links: &links,
        ..TaskRelated::default()
    };

    assert_eq!(names(&task::from_model(&task_row, &related)), task::FIELDS);
    assert_eq!(
        names(&task_detail::from_model(
            &task_row,
            &related,
            &[] as &[TaskRelation],
            &[] as &[TaskComment],
            None,
            None,
        )),
        task_detail::FIELDS
    );
    assert_eq!(
        names(&task_brief::from_model(&task_row)),
        task_brief::FIELDS
    );
    assert_eq!(names(&task_file::from_model(&files[0])), task_file::FIELDS);
    assert_eq!(names(&task_link::from_model(&links[0])), task_link::FIELDS);
    assert_eq!(
        names(&task_rollup::from_model(2, &[("ready".into(), 2)], 0, 0.0)),
        task_rollup::FIELDS
    );
    assert_eq!(
        names(&dependency_state::from_model(true, Vec::new())),
        dependency_state::FIELDS
    );
    assert_eq!(
        names(&graph_node::from_model(&task_row, &[], &[], &tags, false)),
        graph_node::FIELDS
    );
    assert_eq!(
        names(&graph_edge::from_model(
            task_row.id,
            task_row.id,
            graph_edge::EdgeKind::Relation,
            Some(RelationType::RelatesTo.as_str()),
        )),
        graph_edge::FIELDS
    );
    assert_eq!(
        names(&task_graph::from_model(Vec::new(), Vec::new())),
        task_graph::FIELDS
    );
    assert_eq!(
        names(&page::from_model(Vec::new(), 0, 50, 0, None)),
        page::FIELDS
    );
}

#[test]
fn every_comment_relation_and_decision_dto_lists_its_fields_in_order() {
    let task_row = task_row();
    let comment_row = TaskComment {
        id: id_of("00000000-0000-4000-8000-000000000040"),
        task_id: task_row.id,
        attempt_id: None,
        author: "human:Brice".into(),
        kind: CommentKind::Comment,
        body: "hi".into(),
        data: JsonObject::new(),
        created_at: at(0),
    };
    let relation_row = TaskRelation {
        id: id_of("00000000-0000-4000-8000-000000000050"),
        source_task_id: task_row.id,
        target_task_id: task_row.id,
        relation_type: RelationType::RelatesTo,
        created_by: "human:Brice".into(),
        created_at: at(0),
    };
    let decision = decision_row();

    assert_eq!(names(&comment::from_model(&comment_row)), comment::FIELDS);
    assert_eq!(
        names(&relation::from_model(&relation_row)),
        relation::FIELDS
    );
    assert_eq!(
        names(&decision_option::from_model(&decision.options[0])),
        decision_option::FIELDS
    );
    assert_eq!(
        names(&decision_task::from_model(&task_row, &[])),
        decision_task::FIELDS
    );
    assert_eq!(
        names(&decision::from_model(&decision, "WG-7")),
        decision::FIELDS
    );
    let task_value = decision_task::from_model(&task_row, &[]);
    assert_eq!(
        names(&decision_with_task::from_model(
            &decision, "WG-7", task_value
        )),
        decision_with_task::FIELDS
    );
    assert_eq!(
        decision_option_in::FIELDS,
        ["id", "label", "detail", "recommended"]
    );
    assert_eq!(decision_request::FIELDS, ["question", "options"]);
    assert_eq!(decision_choose::FIELDS, ["option_id", "other_text", "note"]);
    assert_eq!(decision_withdraw::FIELDS, ["reason"]);
}

#[test]
fn a_task_prints_with_the_fields_model_dump_gives() {
    let mut task_row = task_row();
    task_row.category = Some("documentation".into());
    task_row.estimated_effort = Some(Map::from_iter([("value".to_string(), json!(3))]));
    let text = http_json(&task::from_model(&task_row, &TaskRelated::default())).expect("dump");
    assert!(text.starts_with(
        r#"{"id":"00000000-0000-4000-8000-000000000010","workspace_id":"00000000-0000-4000-8000-000000000001","key":"WG-7","number":7,"title":"Wire DTOs","description":"d","acceptance_criteria":["a"],"status":"ready","priority":"high","category":"documentation","tags":[],"files":[],"project_ids":[],"repository_ids":[],"parent_id":null,"child_ids":[],"depends_on":[],"blocks":[],"assignee":null,"assigned_agent":null,"claimed_by":null,"claim_expires_at":null,"auto_execute":false,"estimated_effort":{"value":3},"links":[],"metadata":{},"due_at":null,"scheduled_at":null,"created_by":"human:Brice","version":3,"created_at":"2026-10-25T13:56:00Z","updated_at":"2026-10-25T13:56:00.250000Z","started_at":null,"completed_at":null,"accepted_at":null,"archived_at":null,"current_classification_id":null}"#
    ));
}

#[test]
fn decision_out_carries_the_task_key_and_options() {
    let decision = decision_row();
    let text = http_json(&decision::from_model(&decision, "WG-7")).expect("dump");
    assert!(text.contains(
        r#""options":[{"id":"a","label":"A","detail":"","recommended":true}],"status":"open""#
    ));
    assert!(text.contains(r#""task_key":"WG-7""#));
}
