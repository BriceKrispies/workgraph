//! WG-3738: the MCP output shapes of all 69 tools against the recorded transcripts, the stored
//! event row, the timestamp form of the dict-shaped HTTP outputs, and the field order of the dict
//! DTOs against the Python dict builders.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{Value as Json, json};
use wg_json::{Value, http_json};
use wg_model::entity::JsonObject;
use wg_model::entity::queue::{Job, JobRun, Worker};
use wg_model::entity::run::RevisionManifest;
use wg_types::{Id, JobStatus, Timestamp, parse_id};
use wg_wire::contract::mcp_tools;
use wg_wire::dto::mcp_outputs::{self, output_shape, tools_with_shape};
use wg_wire::dto::{attempt_dicts, event, job, scheduler, worker};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn id_of(text: &str) -> Id {
    parse_id(text).expect("id")
}

fn at(micros: u32) -> Timestamp {
    Timestamp::from_components(2026, 10, 26, 4, 55, 43, micros).expect("timestamp")
}

fn printed(value: &Value) -> Json {
    serde_json::from_str(&http_json(value).expect("dump")).expect("json")
}

fn keys(value: &Json) -> Vec<String> {
    value.as_object().expect("object").keys().cloned().collect()
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

/// Every record of both MCP transcripts.
fn records() -> Vec<(String, Json)> {
    ["stdio", "http"]
        .iter()
        .flat_map(|transport| {
            let path = root().join(format!("contracts/mcp/{transport}.jsonl"));
            fs::read_to_string(path)
                .expect("transcript")
                .lines()
                .map(|line| {
                    (
                        (*transport).to_string(),
                        serde_json::from_str::<Json>(line).expect("record"),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The parsed output of a successful tool call, or `None` for any other record.
fn tool_output(record: &Json) -> Option<(String, Json)> {
    if !matches!(record["kind"].as_str(), Some("ok" | "setup")) || record["outcome"] != "success" {
        return None;
    }
    let tool = record["tool"].as_str()?.to_string();
    let result = &record["response"]["result"];
    if result["isError"] == true {
        return None;
    }
    let body = result["content"][0]["text"].as_str()?;
    Some((tool, serde_json::from_str(body).ok()?))
}

/// The decision, automation and registry tools: their output shapes are the features'
/// (`wg_decisions::spec::shapes`, `wg_automation_feature::spec::shapes`,
/// `wg_registry_feature::spec::shapes`), not this catalog's, so the catalog checks leave them
/// out (docs/rust-rewrite.md 4.16).
const FEATURE_SHAPED: [&str; 13] = [
    "task_request_decision",
    "decision_list",
    "task_decisions",
    "automation_list",
    "automation_trigger",
    "workspace_list",
    "workspace_get",
    "project_list",
    "project_get",
    "repository_list",
    "repository_get",
    "repository_register",
    "repository_status",
];

#[test]
fn every_tool_has_exactly_one_output_shape() {
    let tools: BTreeSet<String> = mcp_tools()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name").to_string())
        .collect();
    let shaped: BTreeSet<String> = tools_with_shape().map(str::to_string).collect();
    assert_eq!(tools.len(), 69);
    let base: BTreeSet<String> = tools
        .into_iter()
        .filter(|tool| !FEATURE_SHAPED.contains(&tool.as_str()))
        .collect();
    assert_eq!(
        base, shaped,
        "tools without a shape or shapes without a tool"
    );
    assert_eq!(
        tools_with_shape().count(),
        shaped.len(),
        "a tool appears twice in the table"
    );
}

#[test]
fn recorded_tool_outputs_match_their_shapes() {
    let mut covered = BTreeSet::new();
    let mut checked = 0;
    for (transport, record) in records() {
        let Some((tool, output)) = tool_output(&record) else {
            continue;
        };
        if !FEATURE_SHAPED.contains(&tool.as_str()) {
            let shape = output_shape(&tool).unwrap_or_else(|| panic!("no shape for {tool}"));
            shape.check(&output).unwrap_or_else(|failure| {
                panic!(
                    "{transport} {}: {failure}",
                    record["label"].as_str().unwrap_or("?")
                )
            });
        }
        covered.insert(tool);
        checked += 1;
    }
    assert!(checked >= 69, "only {checked} recorded outputs checked");
    assert_eq!(covered.len(), 69, "every tool has a recorded success");
}

#[test]
fn a_shape_names_the_first_difference() {
    let shape = output_shape("stack_verify").expect("shape");
    assert!(shape.check(&json!({"stack": "A", "job_id": "b"})).is_ok());
    let failure = shape
        .check(&json!({"job_id": "b", "stack": "A"}))
        .expect_err("order matters");
    assert!(failure.contains("keys"), "{failure}");
    let failure = output_shape("task_list")
        .expect("shape")
        .check(&json!({"items": [1], "total": 1, "limit": 1, "offset": 0, "next_offset": null}))
        .expect_err("item is not a task");
    assert!(failure.contains("$.items[0]"), "{failure}");
}

#[test]
fn wrapper_builders_produce_the_shapes_the_tools_declare() {
    let cases: Vec<(&str, Value)> = vec![
        ("stack_history", mcp_outputs::events(Vec::new())),
        ("task_get_history", mcp_outputs::events(Vec::new())),
        ("stack_list", mcp_outputs::stacks(Vec::new())),
        ("task_get_ready", mcp_outputs::tasks(Vec::new())),
        ("task_link_commit", mcp_outputs::links(Vec::new())),
        (
            "stack_integrate",
            mcp_outputs::stack_integrate("DEMO-8", &[("auth-service", "job-1")]),
        ),
        ("stack_verify", mcp_outputs::stack_verify("DEMO-8", "job-2")),
        (
            "task_promote_ready",
            mcp_outputs::promote_ready(&["DEMO-1"], &[("(none)".to_string(), 1)], true),
        ),
        (
            "attempt_start",
            mcp_outputs::attempt_start(
                "a",
                1,
                "DEMO-2",
                vec![mcp_outputs::started_repository(
                    "r",
                    "/tmp/r",
                    Some("wg/demo-2"),
                    Some("abc"),
                    Value::Null,
                )],
                Vec::new(),
            ),
        ),
        (
            "attempt_submit",
            mcp_outputs::submit_failed("failed", Some("boom"), "failed", "failed"),
        ),
        (
            "attempt_submit",
            mcp_outputs::submit_succeeded(Value::Null, Value::Null, "review", "succeeded"),
        ),
        (
            "task_import_breakdown",
            mcp_outputs::import_breakdown(
                "doc#Plan",
                true,
                vec![mcp_outputs::breakdown_row(
                    "CF-1",
                    None,
                    "Title",
                    "created",
                    "ready",
                    &[],
                    &["t"],
                )],
                &[("CF-1", None)],
                vec![mcp_outputs::breakdown_unresolved("CF-1", "X-1", "no task")],
                1,
                0,
                0,
                1,
                0,
            ),
        ),
    ];
    for (tool, value) in cases {
        output_shape(tool)
            .expect("shape")
            .check(&printed(&value))
            .unwrap_or_else(|failure| panic!("{tool}: {failure}"));
    }
}

#[test]
fn simple_wrappers_print_the_recorded_bytes() {
    // The recorded text is `json.dumps(indent=2)`; the CLI dialect prints the same form.
    let recorded = |label: &str| -> Json {
        records()
            .into_iter()
            .find(|(_, record)| record["label"] == label)
            .and_then(|(_, record)| tool_output(&record).map(|(_, output)| output))
            .unwrap_or_else(|| panic!("no recording {label}"))
    };
    let verify = recorded("stack_verify:ok");
    let value = mcp_outputs::stack_verify(
        verify["stack"].as_str().expect("stack"),
        verify["job_id"].as_str().expect("job"),
    );
    assert_eq!(http_json(&value).expect("dump"), verify.to_string());

    let integrate = recorded("stack_integrate:ok");
    let jobs: Vec<(&str, &str)> = integrate["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .map(|job| {
            (
                job["repository"].as_str().expect("repository"),
                job["job_id"].as_str().expect("job"),
            )
        })
        .collect();
    let value = mcp_outputs::stack_integrate(integrate["stack"].as_str().expect("stack"), &jobs);
    assert_eq!(http_json(&value).expect("dump"), integrate.to_string());

    let promote = recorded("task_promote_ready:ok");
    let promoted: Vec<&str> = promote["promoted"]
        .as_array()
        .expect("promoted")
        .iter()
        .map(|key| key.as_str().expect("key"))
        .collect();
    let by_project: Vec<(String, i64)> = promote["by_project"]
        .as_object()
        .expect("by_project")
        .iter()
        .map(|(slug, count)| (slug.clone(), count.as_i64().expect("count")))
        .collect();
    let value = mcp_outputs::promote_ready(
        &promoted,
        &by_project,
        promote["dry_run"].as_bool().expect("dry_run"),
    );
    assert_eq!(http_json(&value).expect("dump"), promote.to_string());
}

// ---------------------------------------------------------------------------- stored events

/// A stored event from the recorded JSON of one event.
fn stored_from(recorded: &Json) -> wg_model::StoredEvent {
    let text = |key: &str| recorded[key].as_str().expect(key).to_string();
    let optional_id = |key: &str| recorded[key].as_str().map(id_of);
    wg_model::StoredEvent {
        seq: recorded["seq"].as_i64().expect("seq"),
        id: id_of(&text("id")),
        workspace_id: optional_id("workspace_id"),
        event_type: text("event_type"),
        entity_type: text("entity_type"),
        entity_id: id_of(&text("entity_id")),
        actor: text("actor"),
        actor_kind: text("actor_kind"),
        occurred_at: Timestamp::parse_iso(&text("occurred_at")).expect("timestamp"),
        correlation_id: id_of(&text("correlation_id")).into(),
        causation_id: optional_id("causation_id"),
        attempt_id: optional_id("attempt_id"),
        schema_version: recorded["schema_version"].as_i64().expect("version"),
        data: recorded["data"].as_object().expect("data").clone(),
    }
}

fn first_event(label: &str, pointer: &str) -> Json {
    let (_, record) = records()
        .into_iter()
        .find(|(_, record)| record["label"] == label)
        .unwrap_or_else(|| panic!("no recording {label}"));
    let (_, output) = tool_output(&record).expect("output");
    output
        .pointer(pointer)
        .unwrap_or_else(|| panic!("{label}: no {pointer}"))
        .clone()
}

#[test]
fn event_out_prints_the_recorded_history_row() {
    for (label, pointer) in [
        ("stack_history:ok", "/events/0"),
        ("task_get_history:ok", "/events/0"),
        ("task_get_history:ok", "/events/3"),
    ] {
        let recorded = first_event(label, pointer);
        let value = event::from_stored(&stored_from(&recorded));
        assert_eq!(
            http_json(&value).expect("dump"),
            recorded.to_string(),
            "{label} {pointer}"
        );
        assert_eq!(keys(&printed(&value)), strings(&event::FIELDS));
    }
}

#[test]
fn run_get_events_are_dicts_with_isoformat_timestamps() {
    let recorded = first_event("run_get:ok", "/recent_events/0");
    assert!(
        recorded["occurred_at"]
            .as_str()
            .expect("text")
            .ends_with("+00:00"),
        "the recording shows the isoformat form"
    );
    let value = event::dict_from_stored(&stored_from(&recorded));
    assert_eq!(http_json(&value).expect("dump"), recorded.to_string());
}

#[test]
fn a_drafted_event_reads_back_with_sorted_data() {
    use wg_model::event::project::ProjectCreated;
    use wg_model::{EventDraft, EventPayload};
    use wg_types::{Actor, ActorKind};
    let draft = EventDraft {
        id: id_of("00000000-0000-4000-8000-0000000000d1"),
        workspace_id: None,
        entity_id: id_of("00000000-0000-4000-8000-0000000000d2"),
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
    let stored = wg_model::StoredEvent::from_draft(3, &draft).expect("stored");
    let order: Vec<&str> = stored.data.keys().map(String::as_str).collect();
    assert_eq!(order, ["name", "repository_ids", "slug"]);
}

// ---------------------------------------------------------------------------- dict timestamps

fn sample_job() -> Job {
    Job {
        id: id_of("00000000-0000-4000-8000-0000000000a1"),
        workspace_id: Some(id_of("00000000-0000-4000-8000-000000000001")),
        kind: "noop".into(),
        status: JobStatus::Queued,
        priority: 100,
        payload: JsonObject::from_iter([("a".to_string(), json!(1))]),
        result: None,
        run_at: at(38_933),
        attempts: 0,
        max_attempts: 5,
        backoff_base_seconds: wg_model::entity::Real(2.0),
        timeout_seconds: None,
        lease_owner: None,
        lease_expires_at: None,
        heartbeat_at: Some(at(0)),
        started_at: None,
        finished_at: None,
        cancel_requested: false,
        idempotency_key: None,
        resource_keys: std::iter::empty().collect(),
        last_error: None,
        error_kind: None,
        correlation_id: None,
        causation_id: None,
        task_id: None,
        attempt_id: None,
        created_by: "system".into(),
        created_at: at(38_933),
        updated_at: at(38_933),
    }
}

fn sample_worker() -> Worker {
    Worker {
        id: "w1".into(),
        hostname: "host".into(),
        pid: 123,
        status: "running".into(),
        concurrency: 2,
        kinds: std::iter::empty().collect(),
        current_jobs: std::iter::empty().collect(),
        jobs_completed: 0,
        jobs_failed: 0,
        started_at: at(54_556),
        heartbeat_at: at(54_556),
        stopped_at: None,
    }
}

#[test]
fn job_and_worker_dicts_print_the_z_form_a_live_python_response_gave() {
    // Recorded from a live Python response (GET /jobs, /jobs/{id}, /workers): FastAPI encodes a
    // datetime inside a plain dict like a model field.
    let job_text = http_json(&job::from_model(&sample_job())).expect("dump");
    assert!(
        job_text.contains(r#""run_at":"2026-10-26T04:55:43.038933Z""#),
        "{job_text}"
    );
    assert!(
        job_text.contains(r#""heartbeat_at":"2026-10-26T04:55:43Z""#),
        "{job_text}"
    );
    assert!(!job_text.contains("+00:00"), "{job_text}");

    let run = JobRun {
        id: id_of("00000000-0000-4000-8000-0000000000a2"),
        job_id: id_of("00000000-0000-4000-8000-0000000000a1"),
        attempt_number: 1,
        worker_id: "w1".into(),
        status: "succeeded".into(),
        error: None,
        started_at: at(1),
        finished_at: Some(at(2)),
    };
    let detail = http_json(&job::detail_from_model(&sample_job(), &[run])).expect("dump");
    assert!(
        detail.contains(r#""started_at":"2026-10-26T04:55:43.000001Z""#),
        "{detail}"
    );
    assert!(!detail.contains("+00:00"), "{detail}");

    let worker_text = http_json(&worker::from_model(&sample_worker())).expect("dump");
    assert!(
        worker_text.contains(r#""started_at":"2026-10-26T04:55:43.054556Z""#),
        "{worker_text}"
    );
    assert!(
        worker_text.contains(r#""stopped_at":null"#),
        "{worker_text}"
    );
}

#[test]
fn manifests_print_isoformat_timestamps() {
    let row = RevisionManifest {
        id: id_of("00000000-0000-4000-8000-0000000000b1"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        attempt_id: None,
        kind: "baseline".into(),
        entries: [JsonObject::from_iter([("sha".to_string(), json!("abc"))])]
            .into_iter()
            .collect(),
        created_at: at(5),
    };
    let text = http_json(&attempt_dicts::manifest(&row)).expect("dump");
    assert_eq!(
        text,
        concat!(
            r#"{"id":"00000000-0000-4000-8000-0000000000b1","kind":"baseline","#,
            r#""entries":[{"sha":"abc"}],"created_at":"2026-10-26T04:55:43.000005+00:00"}"#
        )
    );
}

// ---------------------------------------------------------------------------- dict field order

/// The text of the Python source between two markers.
fn python_block(file: &str, start: &str, end: &str) -> String {
    let text = fs::read_to_string(root().join(file)).expect("python source");
    let from = text
        .find(start)
        .unwrap_or_else(|| panic!("{file}: no {start}"));
    let rest = &text[from..];
    let to = rest[start.len()..]
        .find(end)
        .unwrap_or_else(|| panic!("{file}: no {end} after {start}"));
    rest[..start.len() + to].to_string()
}

/// The quoted names of a tuple of strings, one per line (`"id",`).
fn tuple_names(block: &str) -> Vec<String> {
    block
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let name = line.strip_prefix('"')?.strip_suffix("\",")?;
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                .then(|| name.to_string())
        })
        .collect()
}

/// The keys of dict literals written `"key": value`, in order of appearance.
fn dict_keys(block: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = block;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let name = &after[..close];
        let follows = after[close + 1..].starts_with(':');
        if follows
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            found.push(name.to_string());
        }
        rest = &after[close + 1..];
    }
    found
}

#[test]
fn job_worker_and_scheduler_dicts_follow_the_python_builders() {
    let job_fields = tuple_names(&python_block(
        "src/workgraph/api/routes/ops.py",
        "def _job(",
        "@router.get(\"/jobs\")",
    ));
    assert_eq!(job_fields, strings(&job::FIELDS));

    let run_fields = dict_keys(&python_block(
        "src/workgraph/api/routes/ops.py",
        "def get_job(",
        "return {**_job",
    ));
    assert_eq!(run_fields, strings(&job::RUN_FIELDS));

    let mut detail = strings(&job::FIELDS);
    detail.push("runs".into());
    assert_eq!(detail, strings(&job::DETAIL_FIELDS));

    let worker_fields = tuple_names(&python_block(
        "src/workgraph/api/routes/ops.py",
        "def workers(",
        "for w in app.queue.workers()",
    ));
    assert_eq!(worker_fields, strings(&worker::FIELDS));

    let scheduler_fields = dict_keys(&python_block(
        "src/workgraph/api/routes/ops.py",
        "def scheduler(",
        "# ----",
    ));
    assert_eq!(scheduler_fields, strings(&scheduler::FIELDS));

    let eligible = dict_keys(&python_block(
        "src/workgraph/orchestration/scheduler.py",
        "eligible.append(",
        "return self._fair_order",
    ));
    assert_eq!(eligible, strings(&scheduler::ELIGIBLE_FIELDS));

    let manifest = dict_keys(&python_block(
        "src/workgraph/core/services/attempts.py",
        "def manifests(",
        "def compare(",
    ));
    assert_eq!(manifest, strings(&attempt_dicts::MANIFEST_FIELDS));

    let compare = python_block(
        "src/workgraph/core/services/attempts.py",
        "            return {\n                \"a\": a.id,",
        "}\n",
    );
    assert_eq!(dict_keys(&compare), strings(&attempt_dicts::COMPARE_FIELDS));

    let log = dict_keys(&python_block(
        "src/workgraph/api/routes/ops.py",
        "return {\"lines\": lines[-tail:]",
        "\n",
    ));
    assert_eq!(log, strings(&attempt_dicts::LOG_FIELDS));
}

#[test]
fn printed_dicts_list_their_fields_in_order() {
    assert_eq!(
        keys(&printed(&job::from_model(&sample_job()))),
        strings(&job::FIELDS)
    );
    assert_eq!(
        keys(&printed(&job::detail_from_model(&sample_job(), &[]))),
        strings(&job::DETAIL_FIELDS)
    );
    assert_eq!(
        keys(&printed(&worker::from_model(&sample_worker()))),
        strings(&worker::FIELDS)
    );
    let eligible = scheduler::eligible_entry("t", "WG-1", "medium", None, None, "w");
    assert_eq!(
        keys(&printed(&eligible)),
        strings(&scheduler::ELIGIBLE_FIELDS)
    );
    assert_eq!(
        keys(&printed(&scheduler::from_parts(
            false,
            false,
            vec![eligible]
        ))),
        strings(&scheduler::FIELDS)
    );
}
