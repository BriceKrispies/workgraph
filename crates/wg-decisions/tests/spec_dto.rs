//! The decision DTOs (LIB-13, moved with the feature): each DTO built from model values lists its
//! fields in the order of its `FIELDS` constant, and the values print as the API prints them.

#![allow(clippy::expect_used)]

use rpds::VectorSync;

use wg_decisions::spec::dto::{
    decision, decision_choose, decision_option, decision_option_in, decision_request,
    decision_task, decision_with_task, decision_withdraw,
};
use wg_json::{Value, http_json};
use wg_model::entity::JsonObject;
use wg_model::entity::task::{DecisionOption, Task, TaskDecision};
use wg_types::{Id, TaskKey, TaskPriority, TaskStatus, Timestamp, parse_id};

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
fn every_decision_dto_lists_its_fields_in_order() {
    let task_row = task_row();
    let decision = decision_row();
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
fn decision_out_carries_the_task_key_and_options() {
    let decision = decision_row();
    let text = http_json(&decision::from_model(&decision, "WG-7")).expect("dump");
    assert!(text.contains(
        r#""options":[{"id":"a","label":"A","detail":"","recommended":true}],"status":"open""#
    ));
    assert!(text.contains(r#""task_key":"WG-7""#));
}
