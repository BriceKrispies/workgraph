//! LIB-17: the settings, agent, llm, claude status, meta, health and event DTOs. The field orders
//! are checked against the Python source and the recorded MCP outputs, and the timestamp forms
//! against what each Python handler prints. The automation and notification DTOs moved with their
//! feature (`wg-automation-feature/tests/spec_dto.rs`).

#![allow(clippy::expect_used)]

use wg_json::{PyStr, Value, http_json};
use wg_model::entity::session::AgentSession;
use wg_model::event::project::ProjectCreated;
use wg_model::event::{EventDraft, EventPayload};
use wg_types::{Actor, ActorKind, Id, Timestamp, parse_id};

use wg_wire::dto::{agents, claude, event, health, llm};

fn id_of(text: &str) -> Id {
    parse_id(text).expect("id")
}

fn at(micros: u32) -> Timestamp {
    Timestamp::from_components(2026, 10, 25, 13, 56, 0, micros).expect("timestamp")
}

/// A dict key as text (keys are stored as code points).
fn key_text(key: &PyStr) -> String {
    key.code_points()
        .iter()
        .map(|cp| char::from_u32(*cp).expect("code point"))
        .collect()
}

fn names(value: &Value) -> Vec<String> {
    match value {
        Value::Model { fields, .. } => fields.iter().map(|(k, _)| k.clone()).collect(),
        Value::Dict(fields) => fields.iter().map(|(k, _)| key_text(k)).collect(),
        other => panic!("neither a model nor a dict: {other:?}"),
    }
}

#[test]
fn events_carry_the_stored_columns_in_python_order() {
    let draft = EventDraft {
        id: id_of("00000000-0000-4000-8000-0000000000d1"),
        workspace_id: Some(id_of("00000000-0000-4000-8000-000000000001")),
        entity_id: id_of("00000000-0000-4000-8000-0000000000d2"),
        entity_text: None,
        entity_type_override: None,
        actor: Actor::new(ActorKind::Human, "Brice").expect("actor"),
        occurred_at: at(0),
        correlation_id: id_of("00000000-0000-4000-8000-0000000000d3").into(),
        causation_id: None,
        attempt_id: None,
        automation_depth: 0,
        payload: EventPayload::ProjectCreated(ProjectCreated {
            slug: "p".into(),
            name: "P".into(),
            repository_ids: std::iter::empty().collect(),
        }),
    };
    let value = event::from_model(7, &draft).expect("event");
    assert_eq!(names(&value), event::EventOut::FIELDS);
    let text = http_json(&value).expect("dump");
    assert!(text.contains(r#""seq":7"#), "{text}");
    assert!(text.contains(r#""actor":"human:Brice""#), "{text}");
    assert!(text.contains(r#""actor_kind":"human""#), "{text}");
    assert!(text.contains(r#""event_type":"project.created""#), "{text}");
    assert!(text.contains(r#""entity_type":"project""#), "{text}");
    assert!(
        text.contains(r#""occurred_at":"2026-10-25T13:56:00Z""#),
        "{text}"
    );
    assert!(text.contains(r#""causation_id":null"#), "{text}");
    // The `data` column is stored with sorted keys, so it reads back sorted.
    assert!(
        text.contains(r#""data":{"name":"P","repository_ids":[],"slug":"p"}"#),
        "{text}"
    );
}

#[test]
fn session_rows_print_isoformat_timestamps() {
    let session = AgentSession {
        id: id_of("00000000-0000-4000-8000-0000000000e1"),
        provider: "claude_code".into(),
        external_session_id: "ext".into(),
        workspace_id: None,
        attempt_id: None,
        task_id: None,
        binding_source: "unbound".into(),
        cwd: None,
        model: None,
        transcript_path: None,
        started_at: Some(at(5)),
        ended_at: None,
        end_reason: None,
        tool_calls: 2,
        tool_failures: 1,
        created_at: at(0),
        updated_at: at(0),
    };
    let value = claude::session(&session);
    assert_eq!(names(&value), claude::SessionOut::FIELDS);
    let text = http_json(&value).expect("dump");
    assert!(
        text.contains(r#""started_at":"2026-10-25T13:56:00.000005+00:00""#),
        "{text}"
    );
    assert!(text.contains(r#""ended_at":null"#), "{text}");
}

#[test]
fn claude_status_document_and_its_parts() {
    let installed = vec!["SessionStart".to_string()];
    let missing = vec!["Stop".to_string()];
    let user = claude::installer_status(
        "user",
        "C:/Users/x/.claude/settings.json",
        true,
        None,
        &installed,
        &missing,
        true,
    );
    assert_eq!(names(&user), claude::InstallerStatus::FIELDS);
    let hooks = claude::hook_stats(4, Some(at(9)), &[("PreToolUse".to_string(), 3)], 0, 0);
    assert_eq!(names(&hooks), claude::HookStats::FIELDS);
    assert!(
        http_json(&hooks)
            .expect("dump")
            .contains(r#""last_received_at":"2026-10-25T13:56:00.000009+00:00""#)
    );
    let status = claude::status(user, hooks, Vec::new());
    assert_eq!(names(&status), claude::ClaudeStatus::FIELDS);
}

#[test]
fn health_and_meta_documents() {
    let report = health::health("0.1.0");
    assert_eq!(names(&report), health::HealthOut::FIELDS);
    assert_eq!(
        http_json(&report).expect("dump"),
        r#"{"status":"ok","version":"0.1.0"}"#
    );

    let meta = health::meta(
        "0.1.0",
        Some("default"),
        true,
        false,
        &["claude".to_string()],
        Value::Dict(Vec::new()),
        Value::Dict(Vec::new()),
    );
    assert_eq!(names(&meta), health::MetaOut::FIELDS);
}

#[test]
fn agents_and_llm_health_entries() {
    let entry = agents::agent(
        "claude",
        "command",
        true,
        "/usr/bin/claude",
        Value::Dict(Vec::new()),
        Value::Dict(Vec::new()),
    );
    assert_eq!(names(&entry), agents::AgentEntry::FIELDS);

    let health = llm::health([("local", true, "3 models")]);
    match &health {
        Value::Dict(fields) => {
            assert_eq!(fields.len(), 1);
            assert_eq!(names(&fields[0].1), llm::Entry::FIELDS);
        }
        other => panic!("not a dict: {other:?}"),
    }
}
