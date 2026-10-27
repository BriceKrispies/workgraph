//! WG-3854: the `agent_run` job against a real database with a fake agent adapter: a successful
//! run is concluded and handed to review, a failed one fails its attempt and task (and escalates
//! after the limit), a cancelled one frees the task, and the agent's events and log are recorded.
//! No agent process is started.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;

use serde_json::{Map, Value, json};
use wg_agents::{AgentError, AgentEvent, AgentRunRequest, AgentRunResult, EventKind, RunStatus};
use wg_exec_feature::service::agents::AgentHost;
use wg_exec_feature::service::conclude::{Collaborators, Live, Unported};
use wg_exec_feature::service::execute::execute;
use wg_exec_feature::service::run_flow::run_start;
use wg_exec_feature::spec::runs::StartRequest;
use wg_jobs::queue::ClaimedJob;
use wg_model::entity::{AgentAttempt, Task, Workspace};
use wg_model::{Decision, RowKey, Write, derive_id};
use wg_proc::Cancel;
use wg_runtime::{App, JobContext, human_context};
use wg_store::EventQuery;
use wg_types::{AttemptStatus, Id, TaskKey, TaskPriority, TaskStatus, Timestamp};

/// A fake adapter: it is available, emits its events, and answers with its result.
struct Fake {
    available: bool,
    events: Vec<AgentEvent>,
    result: AgentRunResult,
    honor_cancel: bool,
    requests: Vec<AgentRunRequest>,
}

impl Fake {
    fn answering(result: AgentRunResult) -> Self {
        Self {
            available: true,
            events: Vec::new(),
            result,
            honor_cancel: false,
            requests: Vec::new(),
        }
    }
}

impl AgentHost for Fake {
    fn probe(&mut self, name: &str, _entry: &Map<String, Value>) -> (bool, String) {
        (self.available && name == "fake", "fake adapter".to_owned())
    }

    fn launch(
        &mut self,
        _name: &str,
        _entry: &Map<String, Value>,
        request: &AgentRunRequest,
        mut on_event: Box<dyn FnMut(AgentEvent) + Send>,
        cancel: Option<Cancel<'_>>,
    ) -> Result<AgentRunResult, AgentError> {
        self.requests.push(request.clone());
        if let Some(dir) = request.log_path.parent() {
            std::fs::create_dir_all(dir).expect("log dir");
        }
        std::fs::write(&request.log_path, "agent output\n").expect("log written");
        for event in self.events.clone() {
            on_event(event);
        }
        if self.honor_cancel && cancel.is_some_and(|probe| probe()) {
            return Ok(AgentRunResult::new(RunStatus::Cancelled, None));
        }
        Ok(self.result.clone())
    }
}

fn at() -> Timestamp {
    Timestamp::parse_iso("2026-10-26T09:00:00Z").expect("a timestamp")
}

fn id(n: u32) -> Id {
    derive_id(&[5; 32], n)
}

fn task(n: u32) -> Task {
    Task {
        id: id(10 + n),
        workspace_id: id(0),
        number: i64::from(n),
        key: TaskKey::parse(&format!("WG-{n}")).expect("a key"),
        title: format!("task {n}"),
        description: "do the thing".into(),
        description_hash: "h".into(),
        acceptance_criteria: Default::default(),
        status: TaskStatus::Ready,
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

fn home(name: &str, max_attempts: i64) -> (App, PathBuf) {
    let dir = std::env::temp_dir().join(format!("wg3854x-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::open(Some(&dir)).expect("the app opens");
    let mut config = app.config().expect("config");
    config["execution"]["enabled"] = json!(true);
    config["execution"]["max_attempts_per_task"] = json!(max_attempts);
    config["agents"]["fake"] = json!({"type": "command", "executable": "fake", "args": []});
    app.save_config(&config).expect("config saved");
    app.runtime
        .store
        .commit_decision(
            &Decision::new(())
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
                .write(Write::insert(task(1))),
        )
        .expect("fixtures written");
    (app, dir)
}

fn ctx() -> wg_runtimespec::RequestContext {
    human_context(Some("brice")).expect("context")
}

/// Queues a run of WG-1 and returns its attempt id.
fn queue_run(app: &mut App) -> Id {
    let mut host = Fake::answering(AgentRunResult::new(RunStatus::Succeeded, Some(0)));
    let request = StartRequest {
        agent: Some("fake".into()),
        ..StartRequest::default()
    };
    let out = run_start(app, &ctx(), &mut host, None, "WG-1", &request).expect("start");
    let text = wg_clispec::args::get(&out, "id")
        .map(wg_clispec::args::python)
        .expect("id");
    wg_types::parse_id(&text).expect("an id")
}

fn job_for(attempt: Id) -> JobContext {
    let payload = Map::from_iter([("attempt_id".to_owned(), json!(attempt.to_string()))]);
    JobContext::new(
        ClaimedJob {
            id: id(900),
            kind: "agent_run".into(),
            payload,
            attempt: 1,
            max_attempts: 1,
            workspace_id: Some(id(0)),
            task_id: Some(id(11)),
            attempt_id: Some(attempt),
            correlation_id: None,
            causation_id: None,
            timeout_seconds: None,
            lease_expires_at: at(),
        },
        "worker-1",
    )
}

fn attempt_row(app: &mut App, attempt: Id) -> AgentAttempt {
    app.runtime
        .store
        .read(|s| s.attempt_by_id(attempt))
        .expect("read")
        .expect("exists")
}

fn task_status(app: &mut App) -> TaskStatus {
    app.runtime
        .store
        .read(|s| s.fetch::<Task>(&RowKey::Task(id(11))))
        .expect("task")
        .expect("exists")
        .status
}

fn events(app: &mut App, attempt: Id) -> Vec<String> {
    app.runtime
        .store
        .read(|s| {
            s.query_events(&EventQuery {
                attempt_id: Some(attempt),
                limit: 200,
                ..EventQuery::default()
            })
        })
        .expect("events")
        .iter()
        .map(|record| record.event_type.clone())
        .collect()
}

fn result_of(status: RunStatus) -> AgentRunResult {
    let mut result = AgentRunResult::new(status, Some(0));
    result.summary = "all done".into();
    result.session_id = Some("sess-1".into());
    result.model = Some("model-x".into());
    result.usage = Map::from_iter([("input_tokens".to_owned(), json!(120))]);
    result.cost_usd = Some(0.25);
    result.num_turns = Some(3);
    result
}

#[test]
fn a_successful_run_is_concluded_and_handed_to_review() {
    let (mut app, _dir) = home("success", 3);
    let attempt_id = queue_run(&mut app);
    let mut host = Fake::answering(result_of(RunStatus::Succeeded));
    host.events = vec![
        AgentEvent::with_data(
            EventKind::Session,
            "session",
            json!({"session_id": "sess-1", "model": "model-x"}),
        ),
        AgentEvent::with_data(EventKind::Progress, "working", json!({"percent": 50})),
        AgentEvent::plain(EventKind::Message, "hello"),
    ];
    let mut job = job_for(attempt_id);
    let report = execute(&mut app, &mut host, &mut Unported, &mut job).expect("executes");

    assert_eq!(report["status"], "succeeded");
    assert_eq!(report["verification"], Value::Null);
    assert!(
        report["outcome"]["link_error"]
            .as_str()
            .unwrap()
            .contains("not ported")
    );
    assert_eq!(task_status(&mut app), TaskStatus::AwaitingReview);

    let attempt = attempt_row(&mut app, attempt_id);
    assert_eq!(attempt.status, AttemptStatus::Succeeded);
    assert_eq!(attempt.exit_reason.as_deref(), Some("agent_succeeded"));
    assert_eq!(attempt.exit_code, Some(0));
    assert_eq!(attempt.summary.as_deref(), Some("all done"));
    assert_eq!(attempt.model.as_deref(), Some("model-x"));
    assert_eq!(attempt.session_id.as_deref(), Some("sess-1"));
    assert_eq!(attempt.usage["input_tokens"], 120);
    assert_eq!(attempt.usage_source.as_deref(), Some("command_reported"));
    assert_eq!(attempt.cost_usd.map(|c| c.0), Some(0.25));
    assert_eq!(attempt.cost_source.as_deref(), Some("command_reported"));
    assert_eq!(attempt.telemetry["num_turns"], 3);
    assert_eq!(attempt.telemetry["progress_events"], 3);
    assert!(attempt.telemetry["measured_duration_ms"].is_number());
    assert!(attempt.telemetry["agent_log_artifact_id"].is_string());
    assert!(attempt.telemetry["job_id"].is_string());
    assert!(attempt.environment["workdir"].is_string());
    assert!(attempt.baseline_manifest_id.is_some());
    assert!(attempt.outcome_manifest_id.is_some());

    // The prompt and the log are artifacts with their files.
    let artifacts = app
        .runtime
        .store
        .read(|s| s.artifacts_of_attempt(attempt_id))
        .expect("artifacts");
    let kinds: Vec<&str> = artifacts.iter().map(|a| a.kind.as_str()).collect();
    assert_eq!(kinds, ["prompt", "agent_log"]);
    assert_eq!(Some(artifacts[0].id), attempt.prompt_artifact_id);
    let root = PathBuf::from(app.paths.artifacts_dir().as_str());
    let prompt = std::fs::read_to_string(root.join(&artifacts[0].path)).expect("prompt file");
    assert!(prompt.starts_with("# Task WG-1: task 1\n"));
    assert_eq!(artifacts[1].size_bytes, 13);
    assert_eq!(host.requests.len(), 1);
    assert_eq!(host.requests[0].task_key, "WG-1");
    assert_eq!(host.requests[0].timeout_seconds, 1800.0);

    let recorded = events(&mut app, attempt_id);
    for expected in [
        "attempt.created",
        "attempt.session",
        "attempt.progress",
        "attempt.message",
        "attempt.outcome_captured",
        "attempt.succeeded",
    ] {
        assert!(
            recorded.contains(&expected.to_owned()),
            "{expected} in {recorded:?}"
        );
    }
    assert_eq!(
        recorded
            .iter()
            .filter(|t| *t == "attempt.status_changed")
            .count(),
        2
    );
    // The agent's session is bound to the attempt.
    let bound = app
        .runtime
        .store
        .read(|s| {
            s.rows(&wg_exec_feature::spec::sql::session_by_external(
                "fake", "sess-1",
            ))
        })
        .expect("session");
    assert_eq!(bound.len(), 1);

    // A job that runs again finds the attempt finished.
    let again =
        execute(&mut app, &mut host, &mut Unported, &mut job_for(attempt_id)).expect("again");
    assert_eq!(again["skipped"], "attempt already succeeded");
}

#[test]
fn a_failed_run_fails_its_attempt_and_task_and_escalates_at_the_limit() {
    let (mut app, _dir) = home("failure", 1);
    let attempt_id = queue_run(&mut app);
    let mut failed = result_of(RunStatus::Failed);
    failed.error = Some("boom".into());
    failed.summary = String::new();
    let mut host = Fake::answering(failed);
    let report =
        execute(&mut app, &mut host, &mut Unported, &mut job_for(attempt_id)).expect("executes");
    assert_eq!(report["status"], "failed");
    assert_eq!(report["error"], "boom");

    let attempt = attempt_row(&mut app, attempt_id);
    assert_eq!(attempt.status, AttemptStatus::Failed);
    assert_eq!(attempt.exit_reason.as_deref(), Some("failed"));
    assert_eq!(attempt.error.as_deref(), Some("boom"));
    assert_eq!(attempt.cost_usd.map(|c| c.0), Some(0.25));
    assert_eq!(task_status(&mut app), TaskStatus::Failed);
    let world = app
        .runtime
        .store
        .read(|s| s.task_world(None, &[]))
        .expect("world");
    assert!(world.tasks.tags_of(id(11)).any(|tag| tag == "needs-review"));
    assert!(events(&mut app, attempt_id).contains(&"attempt.failed".to_owned()));
    let all = app
        .runtime
        .store
        .read(|s| {
            s.query_events(&EventQuery {
                limit: 500,
                ..EventQuery::default()
            })
        })
        .expect("events");
    assert!(all.iter().any(|e| e.event_type == "task.escalated"));
}

#[test]
fn a_cancelled_run_frees_the_task() {
    let (mut app, _dir) = home("cancelled", 3);
    let attempt_id = queue_run(&mut app);
    let mut host = Fake::answering(result_of(RunStatus::Succeeded));
    host.honor_cancel = true;
    let mut job = job_for(attempt_id);
    // The worker raises the job's flag when a cancel arrives.
    job.request_cancel();
    let report = execute(&mut app, &mut host, &mut Unported, &mut job).expect("executes");
    assert_eq!(report["status"], "cancelled");
    let attempt = attempt_row(&mut app, attempt_id);
    assert_eq!(attempt.status, AttemptStatus::Cancelled);
    assert_eq!(attempt.exit_reason.as_deref(), Some("cancelled"));
    assert_eq!(task_status(&mut app), TaskStatus::Ready);
}

#[test]
fn a_run_cancelled_before_it_starts_never_launches_the_agent() {
    let (mut app, _dir) = home("early", 3);
    let attempt_id = queue_run(&mut app);
    // The attempt is flagged while its job waits (cancel on a started run only flags it).
    let mut flagged = attempt_row(&mut app, attempt_id);
    flagged.status = AttemptStatus::Preparing;
    flagged.cancel_requested = true;
    app.runtime
        .store
        .commit_decision(&Decision::new(()).write(Write::update(flagged)))
        .expect("flagged");
    let mut host = Fake::answering(result_of(RunStatus::Succeeded));
    let report =
        execute(&mut app, &mut host, &mut Unported, &mut job_for(attempt_id)).expect("executes");
    assert_eq!(report["status"], "cancelled");
    assert!(host.requests.is_empty());
    let attempt = attempt_row(&mut app, attempt_id);
    assert_eq!(attempt.status, AttemptStatus::Cancelled);
    assert_eq!(attempt.error.as_deref(), Some("cancelled before start"));
    assert_eq!(task_status(&mut app), TaskStatus::Ready);
}

#[test]
fn an_adapter_that_went_missing_fails_the_attempt() {
    let (mut app, _dir) = home("missing", 3);
    let attempt_id = queue_run(&mut app);
    let mut host = Fake::answering(result_of(RunStatus::Succeeded));
    host.available = false;
    let report =
        execute(&mut app, &mut host, &mut Unported, &mut job_for(attempt_id)).expect("executes");
    assert_eq!(report["status"], "failed");
    assert_eq!(
        report["error"],
        "agent adapter 'fake' is unavailable: fake adapter"
    );
    let attempt = attempt_row(&mut app, attempt_id);
    assert_eq!(attempt.status, AttemptStatus::Failed);
    assert_eq!(attempt.exit_reason.as_deref(), Some("provider_unavailable"));
    assert_eq!(task_status(&mut app), TaskStatus::Failed);
    assert!(host.requests.is_empty());
}

#[test]
fn an_adapter_that_crashes_is_preserved_as_a_failed_attempt() {
    struct Crashing;
    impl AgentHost for Crashing {
        fn probe(&mut self, _: &str, _: &Map<String, Value>) -> (bool, String) {
            (true, "crashing".into())
        }
        fn launch(
            &mut self,
            _: &str,
            _: &Map<String, Value>,
            _: &AgentRunRequest,
            _: Box<dyn FnMut(AgentEvent) + Send>,
            _: Option<Cancel<'_>>,
        ) -> Result<AgentRunResult, AgentError> {
            Err(AgentError::Io {
                path: "x".into(),
                source: std::io::Error::other("disk full"),
            })
        }
    }
    let (mut app, _dir) = home("crash", 3);
    let attempt_id = queue_run(&mut app);
    let report = execute(
        &mut app,
        &mut Crashing,
        &mut Unported,
        &mut job_for(attempt_id),
    )
    .expect("executes");
    assert_eq!(report["status"], "failed");
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .starts_with("adapter crashed: AgentError: cannot write x")
    );
    let attempt = attempt_row(&mut app, attempt_id);
    assert_eq!(attempt.status, AttemptStatus::Failed);
    assert_eq!(attempt.telemetry["exit_reason"], "adapter_crash");
}

// covers: C127
#[test]
fn the_live_collaborators_reach_the_commit_and_verify_features() {
    let (mut app, _dir) = home("live", 3);
    let attempt_id = queue_run(&mut app);
    // Linking is served: an attempt without manifests links nothing.
    Live.link_commits(&mut app, &ctx(), attempt_id)
        .expect("linking is served");
    // The verify feature answers: WG-1 has no verification checks, so the run is refused by it,
    // not by a stand-in that names the missing feature.
    let refused = Live
        .verify(&mut app, &ctx(), id(11), attempt_id, false, &|| false)
        .expect_err("no checks apply");
    assert!(
        !refused.message.contains("(verify feature)"),
        "{}",
        refused.message
    );
    // Stacks do not serve `mark_ready` yet.
    let blocked = Live
        .mark_ready(&mut app, &ctx(), id(11))
        .expect_err("not served");
    assert!(blocked.message.contains("stacks"), "{}", blocked.message);
}
