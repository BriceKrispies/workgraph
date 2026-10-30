//! WG-3853: the automation actions that queue work (`enqueue_agent_run`, `run_verification` and
//! `generate_report`) against a real database. The fake agent host answers the availability probe
//! only; nothing is launched.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use rpds::VectorSync;
use serde_json::{Map, Value, json};
use wg_agents::{AgentError, AgentEvent, AgentRunRequest, AgentRunResult};
use wg_automation_feature::service::execute::execute_run;
use wg_exec_feature::service::agents::AgentHost;
use wg_model::entity::{
    AutomationRule, AutomationRun, Job, JsonObject, Repository, Task, TaskRepository, Workspace,
};
use wg_model::{Decision, RowKey, Write, derive_id};
use wg_proc::Cancel;
use wg_runtime::App;
use wg_types::{AccessMode, Id, TaskKey, TaskPriority, TaskStatus, Timestamp};

/// The host of the tests: the `fake` adapter is available; nothing is ever launched.
struct Fake;

impl AgentHost for Fake {
    fn probe(&mut self, name: &str, _entry: &Map<String, Value>) -> (bool, String) {
        (name == "fake", "fake adapter".to_owned())
    }

    fn launch(
        &mut self,
        _name: &str,
        _entry: &Map<String, Value>,
        _request: &AgentRunRequest,
        _on_event: Box<dyn FnMut(AgentEvent) + Send>,
        _cancel: Option<Cancel<'_>>,
    ) -> Result<AgentRunResult, AgentError> {
        Err(AgentError::Unavailable("the fake host runs nothing".into()))
    }
}

fn at() -> Timestamp {
    Timestamp::parse_iso("2026-10-26T09:00:00Z").expect("a timestamp")
}

fn id(n: u32) -> Id {
    derive_id(&[7; 32], n)
}

fn object(value: Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn task(n: u32, status: TaskStatus) -> Task {
    Task {
        id: id(10 + n),
        workspace_id: id(0),
        number: i64::from(n),
        key: TaskKey::parse(&format!("WG-{n}")).expect("a key"),
        title: format!("task {n}"),
        description: "- write parser".into(),
        description_hash: "h".into(),
        acceptance_criteria: Default::default(),
        status,
        priority: TaskPriority::Medium,
        category: None,
        parent_id: None,
        assignee: None,
        assigned_agent: None,
        claimed_by: None,
        claim_expires_at: None,
        auto_execute: false,
        estimated_effort: None,
        metadata: Map::new(),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at(),
        updated_at: at(),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

/// A home with execution on and the `fake` agent configured, a ready task WG-1 linked to two
/// repositories, and a queued run of a rule whose actions are `actions`.
fn home(name: &str, actions: Vec<Value>) -> (App, Id) {
    let dir = std::env::temp_dir().join(format!("wg3853-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::open(Some(&dir)).expect("the app opens");
    let mut config = app.config().expect("config");
    config["execution"]["enabled"] = json!(true);
    config["agents"]["fake"] = json!({"type": "command", "executable": "fake", "args": []});
    app.save_config(&config).expect("config saved");
    let rule = AutomationRule {
        id: id(40),
        workspace_id: id(0),
        name: "queue work".into(),
        description: String::new(),
        enabled: true,
        trigger: object(json!({"type": "event"})),
        conditions: JsonObject::new(),
        actions: actions.iter().cloned().map(object).collect(),
        retry_max_attempts: 3,
        max_depth: 3,
        last_scheduled_at: None,
        next_run_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at(),
        updated_at: at(),
    };
    let run = AutomationRun {
        id: id(41),
        rule_id: rule.id,
        workspace_id: id(0),
        trigger_type: "event".into(),
        trigger_key: "key".into(),
        trigger_event_id: None,
        depth: 0,
        dry_run: false,
        status: "queued".into(),
        matched: true,
        reason: String::new(),
        condition_trace: VectorSync::new_sync(),
        action_results: VectorSync::new_sync(),
        error: None,
        attempts: 0,
        correlation_id: None,
        created_at: at(),
        finished_at: None,
        inputs: object(json!({"task": "WG-1"})),
    };
    let repository = |n: u32| Repository {
        id: id(n),
        workspace_id: id(0),
        name: format!("repo {n}"),
        description: String::new(),
        identity_fingerprint: None,
        default_branch: None,
        tracked_branches: VectorSync::new_sync(),
        access_mode: AccessMode::ReadWrite,
        health_status: "unknown".into(),
        health_detail: JsonObject::new(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: None,
        metadata: JsonObject::new(),
        created_at: at(),
        updated_at: at(),
    };
    let link = |n: u32| TaskRepository {
        task_id: id(11),
        repository_id: id(n),
        access: "read_write".into(),
        created_at: at(),
    };
    let decision = Decision::new(())
        .write(Write::insert(Workspace {
            id: id(0),
            slug: "default".into(),
            name: "Default".into(),
            description: String::new(),
            key_prefix: "WG".into(),
            next_task_number: 10,
            settings: Map::new(),
            archived_at: None,
            created_at: at(),
            updated_at: at(),
        }))
        .write(Write::insert(task(1, TaskStatus::Ready)))
        .write(Write::insert(repository(60)))
        .write(Write::insert(repository(61)))
        .write(Write::insert(link(60)))
        .write(Write::insert(link(61)))
        .write(Write::insert(rule))
        .write(Write::insert(run));
    app.runtime
        .store
        .commit_decision(&decision)
        .expect("fixtures written");
    (app, id(41))
}

/// The action results of a run, in the order they were recorded.
fn results(app: &mut App, run: Id) -> Vec<JsonObject> {
    let row = app
        .runtime
        .store
        .read(|s| s.fetch::<AutomationRun>(&RowKey::AutomationRun(run)))
        .expect("the run reads")
        .expect("the run exists");
    row.action_results.iter().cloned().collect()
}

/// The queued jobs of one kind.
fn jobs_of(app: &mut App, kind: &str) -> Vec<Job> {
    app.runtime
        .store
        .read(|s| s.active_slice(kind))
        .expect("the queue reads")
        .jobs
        .values()
        .cloned()
        .collect()
}

#[test]
fn run_verification_queues_one_verify_job_holding_each_repository_key() {
    let (mut app, run) = home("verify", vec![json!({"type": "run_verification"})]);
    let outcome = execute_run(&mut app, &mut Fake, run).expect("the run succeeds");
    assert_eq!(outcome, json!({"status": "succeeded", "actions": 1}));
    let jobs = jobs_of(&mut app, "verify");
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.payload.get("task_id"), Some(&json!(id(11).to_string())));
    assert_eq!(job.max_attempts, 1);
    assert_eq!(job.task_id, Some(id(11)));
    assert_eq!(job.workspace_id, Some(id(0)));
    let keys: Vec<String> = job.resource_keys.iter().cloned().collect();
    assert_eq!(
        keys,
        vec![format!("verify:{}", id(60)), format!("verify:{}", id(61))]
    );
    let output = &results(&mut app, run)[0];
    assert_eq!(output["output"]["job_id"], json!(job.id.to_string()));
}

#[test]
fn generate_report_queues_a_report_job_for_the_period() {
    let (mut app, run) = home(
        "report",
        vec![json!({"type": "generate_report", "period_days": 3})],
    );
    execute_run(&mut app, &mut Fake, run).expect("the run succeeds");
    let jobs = jobs_of(&mut app, "generate_report");
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.payload.get("workspace"), Some(&json!("default")));
    assert_eq!(job.payload.get("scope"), Some(&json!({"period_days": 3})));
    assert_eq!(job.max_attempts, 2);
}

#[test]
fn generate_report_defaults_to_a_week() {
    let (mut app, run) = home("report-default", vec![json!({"type": "generate_report"})]);
    execute_run(&mut app, &mut Fake, run).expect("the run succeeds");
    let jobs = jobs_of(&mut app, "generate_report");
    assert_eq!(
        jobs[0].payload.get("scope"),
        Some(&json!({"period_days": 7}))
    );
}

#[test]
fn enqueue_agent_run_starts_an_attempt_on_the_named_agent() {
    let (mut app, run) = home(
        "enqueue",
        vec![json!({"type": "enqueue_agent_run", "agent": "fake"})],
    );
    let outcome = execute_run(&mut app, &mut Fake, run).expect("the run succeeds");
    assert_eq!(outcome["status"], json!("succeeded"));
    let attempts = app
        .runtime
        .store
        .read(|s| s.attempts_of_task(id(11), &[]))
        .expect("attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].provider, "fake");
    let output = &results(&mut app, run)[0];
    assert_eq!(
        output["output"]["attempt_id"],
        json!(attempts[0].id.to_string())
    );
}

#[test]
fn enqueue_agent_run_fails_the_run_when_the_agent_is_unavailable() {
    let (mut app, run) = home(
        "enqueue-missing",
        vec![json!({"type": "enqueue_agent_run", "agent": "nobody"})],
    );
    let error = execute_run(&mut app, &mut Fake, run).expect_err("the run fails");
    assert!(error.message.contains("nobody"), "{}", error.message);
    let stored = &results(&mut app, run)[0];
    assert_eq!(stored["status"], json!("failed"));
}

#[test]
fn classify_task_queues_one_classification_job_for_the_task() {
    let (mut app, run) = home("classify", vec![json!({"type": "classify_task"})]);
    let outcome = execute_run(&mut app, &mut Fake, run).expect("the run succeeds");
    assert_eq!(outcome, json!({"status": "succeeded", "actions": 1}));
    let jobs = jobs_of(&mut app, "classify_task");
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.payload.get("task_id"), Some(&json!(id(11).to_string())));
    assert_eq!(job.payload.get("provider"), Some(&Value::Null));
    assert_eq!(job.payload.get("model"), Some(&Value::Null));
    assert_eq!(job.payload.get("promote"), Some(&json!(false)));
    assert_eq!(job.payload.get("reason"), Some(&json!("requested")));
    assert_eq!(job.max_attempts, 4);
    // Python passes the run's correlation (`ctx.correlation_id`) to the job it queues.
    assert!(job.correlation_id.is_some());
    assert_eq!(job.task_id, Some(id(11)));
    assert_eq!(job.workspace_id, Some(id(0)));
    let keys: Vec<String> = job.resource_keys.iter().cloned().collect();
    assert_eq!(keys, vec!["gpu".to_owned()]);
}

#[test]
fn classify_task_records_the_job_and_task_it_queued() {
    let (mut app, run) = home("classify-output", vec![json!({"type": "classify_task"})]);
    execute_run(&mut app, &mut Fake, run).expect("the run succeeds");
    let jobs = jobs_of(&mut app, "classify_task");
    let stored = &results(&mut app, run)[0];
    assert_eq!(stored["status"], json!("succeeded"));
    assert_eq!(stored["type"], json!("classify_task"));
    let output = &stored["output"];
    assert_eq!(output["job_id"], json!(jobs[0].id.to_string()));
    assert_eq!(output["task_id"], json!(id(11).to_string()));
    assert_eq!(output["task_key"], json!("WG-1"));
    assert!(output["status"].is_string(), "{output}");
}
