//! LIB-17: the automation, notification, settings, agent, llm, claude status, meta, health and
//! event DTOs. The field orders are checked against the Python source and the recorded MCP
//! outputs, and the timestamp forms against what each Python handler prints.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value as Json, json};
use wg_json::{PyStr, Value, http_json};
use wg_model::entity::automation::{AutomationRule, AutomationRun, Notification};
use wg_model::entity::session::AgentSession;
use wg_model::event::project::ProjectCreated;
use wg_model::event::{EventDraft, EventPayload};
use wg_types::{Actor, ActorKind, Id, Timestamp, parse_id};
use wg_wire::dto::{agents, automation, claude, event, health, llm, notification, settings};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

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

fn object(value: Json) -> Map<String, Json> {
    value.as_object().expect("object").clone()
}

fn sample_rule() -> AutomationRule {
    AutomationRule {
        id: id_of("00000000-0000-4000-8000-0000000000a1"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        name: "triage".into(),
        description: String::new(),
        enabled: true,
        trigger: object(json!({"type": "event", "event_types": ["task.created"]})),
        conditions: object(
            json!({"predicate": {"field": "task.priority", "op": "eq", "value": "critical"}}),
        ),
        actions: [object(json!({"type": "add_tags", "tags": ["triage"]}))]
            .into_iter()
            .collect(),
        retry_max_attempts: 3,
        max_depth: 3,
        last_scheduled_at: None,
        next_run_at: None,
        created_by: "human:Brice".into(),
        version: 1,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn sample_run() -> AutomationRun {
    AutomationRun {
        id: id_of("00000000-0000-4000-8000-0000000000b1"),
        rule_id: id_of("00000000-0000-4000-8000-0000000000a1"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        trigger_type: "manual".into(),
        trigger_key: "manual:x".into(),
        trigger_event_id: None,
        depth: 1,
        dry_run: false,
        status: "not_matched".into(),
        matched: false,
        reason: "conditions evaluated to false".into(),
        condition_trace: [object(json!({"field": "task.priority", "result": false}))]
            .into_iter()
            .collect(),
        action_results: std::iter::empty().collect(),
        error: None,
        attempts: 0,
        correlation_id: None,
        created_at: at(0),
        finished_at: Some(at(250_000)),
        inputs: Map::new(),
    }
}

fn sample_notification() -> Notification {
    Notification {
        id: id_of("00000000-0000-4000-8000-0000000000c1"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        level: "warning".into(),
        title: "Critical task".into(),
        body: String::new(),
        source: "automation".into(),
        data: object(json!({"task": "WG-1"})),
        read_at: None,
        created_at: at(0),
    }
}

#[test]
fn rule_and_run_field_order_matches_python_and_the_mcp_recording() {
    let rule = automation::rule(&sample_rule());
    assert_eq!(names(&rule), automation::RULE_FIELDS);
    let run = automation::run(&sample_run());
    assert_eq!(names(&run), automation::RUN_FIELDS);

    // The recorded `automation_list` output (CF-03) prints the same keys, in the same order.
    let recorded = recorded_mcp_output("automation_list:ok");
    let rules = recorded["rules"].as_array().expect("rules");
    let runs = recorded["recent_runs"].as_array().expect("recent_runs");
    let keys = |value: &Json| -> Vec<String> {
        value.as_object().expect("object").keys().cloned().collect()
    };
    assert_eq!(
        keys(&rules[0]),
        automation::RULE_FIELDS
            .iter()
            .map(|f| (*f).to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        keys(&runs[0]),
        automation::RUN_FIELDS
            .iter()
            .map(|f| (*f).to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn pydantic_outputs_print_timestamps_in_the_z_form() {
    let text = http_json(&automation::run(&sample_run())).expect("dump");
    assert!(
        text.contains(r#""created_at":"2026-10-25T13:56:00Z""#),
        "{text}"
    );
    assert!(
        text.contains(r#""finished_at":"2026-10-25T13:56:00.250000Z""#),
        "{text}"
    );
    assert!(text.contains(r#""trigger_event_id":null"#), "{text}");
}

#[test]
fn catalog_and_automation_list_are_dicts_in_python_order() {
    let catalog = automation::catalog(
        [("task.created", "Task created")],
        &["add_tags", "notify"],
        &["eq", "ne"],
    );
    assert_eq!(names(&catalog), automation::CATALOG_FIELDS);

    let list = automation::list(
        vec![automation::rule(&sample_rule())],
        vec![automation::run(&sample_run())],
    );
    assert_eq!(names(&list), automation::LIST_FIELDS);
}

#[test]
fn notifications_print_timestamps_as_isoformat() {
    let note = notification::from_model(&sample_notification());
    assert_eq!(names(&note), notification::FIELDS);
    assert_eq!(
        http_json(&note).expect("dump"),
        concat!(
            r#"{"id":"00000000-0000-4000-8000-0000000000c1","level":"warning","#,
            r#""title":"Critical task","body":"","source":"automation","#,
            r#""data":{"task":"WG-1"},"read_at":null,"created_at":"2026-10-25T13:56:00+00:00"}"#
        )
    );
}

#[test]
fn events_carry_the_stored_columns_in_python_order() {
    let draft = EventDraft {
        id: id_of("00000000-0000-4000-8000-0000000000d1"),
        workspace_id: Some(id_of("00000000-0000-4000-8000-000000000001")),
        entity_id: id_of("00000000-0000-4000-8000-0000000000d2"),
        actor: Actor::new(ActorKind::Human, "Brice").expect("actor"),
        occurred_at: at(0),
        correlation_id: id_of("00000000-0000-4000-8000-0000000000d3"),
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
    assert_eq!(names(&value), event::FIELDS);
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
    assert!(text.contains(r#""data":{"slug":"p","name":"P""#), "{text}");
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
    assert_eq!(names(&value), claude::SESSION_FIELDS);
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
    assert_eq!(names(&user), claude::INSTALLER_FIELDS);
    let hooks = claude::hook_stats(4, Some(at(9)), &[("PreToolUse".to_string(), 3)], 0, 0);
    assert_eq!(names(&hooks), claude::HOOK_FIELDS);
    assert!(
        http_json(&hooks)
            .expect("dump")
            .contains(r#""last_received_at":"2026-10-25T13:56:00.000009+00:00""#)
    );
    let status = claude::status(user, hooks, Vec::new());
    assert_eq!(names(&status), claude::FIELDS);
}

#[test]
fn settings_meta_and_health_documents() {
    let config = Value::Dict(Vec::new());
    let get = settings::settings(config.clone(), "cfg.toml", "data");
    assert_eq!(names(&get), settings::FIELDS);
    assert_eq!(names(&get_paths(&get)), settings::PATH_FIELDS);
    assert_eq!(names(&settings::patched(config)), settings::PATCH_FIELDS);

    let report = health::health("0.1.0");
    assert_eq!(names(&report), health::HEALTH_FIELDS);
    assert_eq!(
        http_json(&report).expect("dump"),
        r#"{"status":"ok","version":"0.1.0"}"#
    );

    let meta = health::meta(
        "0.1.0",
        "default",
        true,
        false,
        &["claude".to_string()],
        Value::Dict(Vec::new()),
        Value::Dict(Vec::new()),
    );
    assert_eq!(names(&meta), health::META_FIELDS);
}

/// The `paths` object of a settings document.
fn get_paths(document: &Value) -> Value {
    match document {
        Value::Dict(fields) => fields
            .iter()
            .find(|(key, _)| key_text(key) == "paths")
            .map(|(_, value)| value.clone())
            .expect("paths"),
        other => panic!("not a dict: {other:?}"),
    }
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
    assert_eq!(names(&entry), agents::FIELDS);

    let health = llm::health([("local", true, "3 models")]);
    match &health {
        Value::Dict(fields) => {
            assert_eq!(fields.len(), 1);
            assert_eq!(names(&fields[0].1), llm::ENTRY_FIELDS);
        }
        other => panic!("not a dict: {other:?}"),
    }
}

/// The `output` of the recorded MCP tool call with the given label, parsed.
fn recorded_mcp_output(label: &str) -> Json {
    let text = fs::read_to_string(root().join("contracts/mcp/stdio.jsonl")).expect("stdio.jsonl");
    let record = text
        .lines()
        .map(|line| serde_json::from_str::<Json>(line).expect("jsonl record"))
        .find(|record| record["label"] == label)
        .unwrap_or_else(|| panic!("no recording labelled {label}"));
    let body = record["response"]["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    serde_json::from_str(body).expect("tool output is JSON")
}
