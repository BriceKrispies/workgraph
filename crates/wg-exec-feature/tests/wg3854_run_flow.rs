//! WG-3854: the run flow's entry points against a real database with a fake agent host: a queued
//! run and its job, cancellation, retry and rerun, the scheduler pass, and the child proposal.
//! No agent process is started; the fake host only answers the availability probe.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;

use serde_json::{Map, Value, json};
use wg_agents::{AgentError, AgentEvent, AgentRunRequest, AgentRunResult};
use wg_exec_feature::service::agents::AgentHost;
use wg_exec_feature::service::run_flow::{run_cancel, run_rerun, run_retry, run_start};
use wg_exec_feature::service::scheduler;
use wg_exec_feature::spec::decompose::{Proposal, read_proposal, validate_decomposition};
use wg_exec_feature::spec::prompt::{PromptRepo, build_prompt};
use wg_exec_feature::spec::runs::StartRequest;
use wg_exec_feature::spec::schedule::{Eligible, fair_order};
use wg_iface::cli::args::{get, python};
use wg_model::entity::{AgentAttempt, Task, Workspace};
use wg_model::{Decision, Write, derive_id};
use wg_proc::Cancel;
use wg_runtime::{App, human_context};
use wg_store::EventQuery;
use wg_types::{ErrorCode, Id, JobStatus, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

/// The host of the tests: one adapter, `fake`, that is available; nothing is ever launched.
struct Fake {
    probed: Vec<String>,
}

impl AgentHost for Fake {
    fn probe(&mut self, name: &str, _entry: &Map<String, Value>) -> (bool, String) {
        self.probed.push(name.to_owned());
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
    derive_id(&[3; 32], n)
}

fn task(n: u32, status: TaskStatus, auto: bool) -> Task {
    Task {
        id: id(10 + n),
        workspace_id: id(0),
        number: i64::from(n),
        key: TaskKey::parse(&format!("WG-{n}")).expect("a key"),
        title: format!("task {n}"),
        description: "- write parser
- then add tests
- ship it"
            .into(),
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
        auto_execute: auto,
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

/// A home with execution on, the `fake` agent configured, and tasks WG-1 (ready, auto-execute) and
/// WG-2 (ready, manual).
fn home(name: &str) -> (App, PathBuf) {
    let dir = std::env::temp_dir().join(format!("wg3854-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut app = App::open(Some(&dir)).expect("the app opens");
    let mut config = app.config().expect("config");
    config["execution"]["enabled"] = json!(true);
    config["agents"]["fake"] = json!({"type": "command", "executable": "fake", "args": []});
    app.save_config(&config).expect("config saved");
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
        .write(Write::insert(task(1, TaskStatus::Ready, true)))
        .write(Write::insert(task(2, TaskStatus::Ready, false)));
    app.runtime
        .store
        .commit_decision(&decision)
        .expect("fixtures written");
    (app, dir)
}

fn request() -> StartRequest {
    StartRequest {
        agent: Some("fake".into()),
        ..StartRequest::default()
    }
}

fn attempts(app: &mut App, task: u32) -> Vec<AgentAttempt> {
    app.runtime
        .store
        .read(|s| s.attempts_of_task(id(10 + task), &[]))
        .expect("attempts")
}

fn events_of(app: &mut App, attempt: Id) -> Vec<String> {
    app.runtime
        .store
        .read(|s| {
            s.query_events(&EventQuery {
                attempt_id: Some(attempt),
                limit: 100,
                ..EventQuery::default()
            })
        })
        .expect("events")
        .iter()
        .map(|record| record.event_type.clone())
        .collect()
}

fn status_of(app: &mut App, task: u32) -> TaskStatus {
    app.runtime
        .store
        .read(|s| s.fetch::<Task>(&wg_model::RowKey::Task(id(10 + task))))
        .expect("task")
        .expect("exists")
        .status
}

fn ctx() -> wg_runtime::spec::RequestContext {
    human_context(Some("brice")).expect("context")
}

fn field(value: &wg_json::Value, name: &str) -> String {
    get(value, name).map(python).unwrap_or_default()
}

#[test]
fn start_claims_the_task_queues_an_attempt_and_its_job() {
    let (mut app, _dir) = home("start");
    let mut host = Fake { probed: Vec::new() };
    let out = run_start(&mut app, &ctx(), &mut host, None, "WG-2", &request()).expect("start");
    assert_eq!(field(&out, "status"), "queued");
    assert_eq!(field(&out, "number"), "1");
    assert_eq!(field(&out, "provider"), "fake");
    assert_eq!(field(&out, "adapter"), "command");
    assert_eq!(host.probed, ["fake"]);

    assert_eq!(status_of(&mut app, 2), TaskStatus::Claimed);
    let rows = attempts(&mut app, 2);
    assert_eq!(rows.len(), 1);
    let attempt = &rows[0];
    assert_eq!(attempt.agent_id.as_deref(), Some("agent:fake"));
    assert_eq!(attempt.config_snapshot["prompt_version"], "run-prompt-v1");
    assert_eq!(attempt.config_snapshot["isolation"], "worktree");
    assert_eq!(attempt.input_task_snapshot["key"], "WG-2");

    let (jobs, _) = app
        .runtime
        .store
        .read(|s| {
            s.list_jobs(&wg_jobs::queue::JobFilter {
                kind: Some("agent_run"),
                ..Default::default()
            })
        })
        .expect("jobs");
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(job.max_attempts, 1);
    assert_eq!(job.priority, 50);
    assert_eq!(job.attempt_id, Some(attempt.id));
    assert_eq!(job.resource_keys.len(), 2);
    assert_eq!(job.payload["attempt_id"], attempt.id.to_string());
    assert_eq!(attempt.telemetry["job_id"], job.id.to_string());
    assert!(events_of(&mut app, attempt.id).contains(&"attempt.created".to_owned()));

    // A second run of the task is refused while the first is active.
    let again = run_start(&mut app, &ctx(), &mut host, None, "WG-2", &request());
    let error = again.expect_err("refused");
    assert_eq!(error.code, ErrorCode::InvalidTransition);
    assert_eq!(error.message, "task WG-2 already has an active attempt #1");
}

#[test]
fn start_needs_the_opt_in_and_an_available_adapter() {
    let (mut app, _dir) = home("refuse");
    let mut host = Fake { probed: Vec::new() };
    let unknown = StartRequest {
        agent: Some("nobody".into()),
        ..StartRequest::default()
    };
    let error = run_start(&mut app, &ctx(), &mut host, None, "WG-2", &unknown).expect_err("no");
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(error.message, "agent adapter 'nobody' is not configured");
    // Nothing changed.
    assert_eq!(status_of(&mut app, 2), TaskStatus::Ready);
    assert!(attempts(&mut app, 2).is_empty());

    let mut config = app.config().expect("config");
    config["execution"]["enabled"] = json!(false);
    app.save_config(&config).expect("saved");
    let error = run_start(&mut app, &ctx(), &mut host, None, "WG-2", &request()).expect_err("off");
    assert_eq!(error.code, ErrorCode::PermissionDenied);
}

#[test]
fn cancel_stops_a_queued_run_at_once_and_frees_the_task() {
    let (mut app, _dir) = home("cancel");
    let mut host = Fake { probed: Vec::new() };
    let started = run_start(&mut app, &ctx(), &mut host, None, "WG-2", &request()).expect("start");
    let attempt_id = field(&started, "id");

    let out = run_cancel(&mut app, &ctx(), &attempt_id, "no longer needed").expect("cancel");
    assert_eq!(field(&out, "status"), "cancelled");
    assert_eq!(field(&out, "exit_reason"), "cancelled");
    assert_eq!(field(&out, "error"), "no longer needed");
    assert_eq!(status_of(&mut app, 2), TaskStatus::Ready);
    let row = attempts(&mut app, 2).remove(0);
    assert!(row.cancel_requested);
    let events = events_of(&mut app, row.id);
    assert!(events.contains(&"attempt.cancelled".to_owned()));
    assert!(events.contains(&"attempt.cancel_requested".to_owned()));
    let (jobs, _) = app
        .runtime
        .store
        .read(|s| s.list_jobs(&wg_jobs::queue::JobFilter::default()))
        .expect("jobs");
    assert_eq!(jobs[0].status, JobStatus::Cancelled);
    assert!(jobs[0].cancel_requested);
    assert!(jobs[0].finished_at.is_some());

    // Cancelling a finished attempt changes nothing and records nothing.
    let before = events_of(&mut app, row.id).len();
    let again = run_cancel(&mut app, &ctx(), &attempt_id, "again").expect("again");
    assert_eq!(field(&again, "status"), "cancelled");
    assert_eq!(events_of(&mut app, row.id).len(), before);
}

#[test]
fn retry_and_rerun_start_a_new_attempt_from_the_old_one() {
    let (mut app, _dir) = home("retry");
    let mut host = Fake { probed: Vec::new() };
    let started = run_start(&mut app, &ctx(), &mut host, None, "WG-2", &request()).expect("start");
    let first = field(&started, "id");

    // A run that has not finished cannot be retried.
    let error = run_retry(&mut app, &ctx(), &mut host, &first, None).expect_err("active");
    assert_eq!(error.message, "only finished attempts can be retried");
    run_cancel(&mut app, &ctx(), &first, "stop").expect("cancel");

    let retried =
        run_retry(&mut app, &ctx(), &mut host, &first, Some("be careful")).expect("retry");
    assert_eq!(field(&retried, "number"), "2");
    assert_eq!(field(&retried, "retry_of_attempt_id"), first);
    let rows = attempts(&mut app, 2);
    assert_eq!(rows[0].retries, 1);
    assert_eq!(rows[1].config_snapshot["instructions"], "be careful");
    assert_eq!(rows[1].provider, "fake");

    let second = field(&retried, "id");
    run_cancel(&mut app, &ctx(), &second, "stop").expect("cancel");
    let rerun = run_rerun(&mut app, &ctx(), &mut host, &second).expect("rerun");
    assert_eq!(field(&rerun, "reproduced_baseline"), "True");
    assert_eq!(field(&rerun, "missing_revisions"), "[]");
    let attempt = get(&rerun, "attempt").expect("attempt");
    assert_eq!(field(attempt, "number"), "3");
    assert_eq!(field(attempt, "rerun_of_attempt_id"), second);
}

#[test]
fn the_scheduler_starts_eligible_tasks_and_explains_the_rest() {
    let (mut app, _dir) = home("schedule");
    let mut host = Fake { probed: Vec::new() };
    let listed = scheduler::eligible_tasks(&mut app, None, 50).expect("eligible");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].key, "WG-1");

    let why = scheduler::explain(&mut app, None, "WG-2").expect("explain");
    assert_eq!(why["eligible"], false);
    assert_eq!(why["reasons"], json!(["auto_execute is off"]));
    let ok = scheduler::explain(&mut app, None, "WG-1").expect("explain");
    assert_eq!(ok["eligible"], true);

    // The default agent (claude_code) is not available through the fake host, so the pass reports
    // the error against the task and starts nothing.
    let report = scheduler::tick(&mut app, &ctx(), &mut host).expect("tick");
    assert_eq!(report["started"], json!([]));
    assert_eq!(
        report["errors"]["WG-1"],
        "agent adapter 'claude_code' is unavailable: fake adapter"
    );

    // Giving the task the fake agent lets the pass start it.
    let mut assigned = task(1, TaskStatus::Ready, true);
    assigned.assigned_agent = Some("fake".into());
    app.runtime
        .store
        .commit_decision(&Decision::new(()).write(Write::update(assigned)))
        .expect("assigned");
    let report = scheduler::tick(&mut app, &ctx(), &mut host).expect("tick");
    assert_eq!(report["started"].as_array().map(Vec::len), Some(1));
    assert_eq!(report["started"][0]["task"], "WG-1");
    assert_eq!(status_of(&mut app, 1), TaskStatus::Claimed);
    assert_eq!(attempts(&mut app, 1).len(), 1);
    // The task is no longer eligible: its attempt is active.
    let why = scheduler::explain(&mut app, None, "WG-1").expect("explain");
    assert_eq!(
        why["reasons"],
        json!(["status is claimed", "attempt already active"])
    );
}

#[test]
fn decompose_proposes_and_creates_children_in_dependency_order() {
    let (mut app, _dir) = home("decompose");
    let dry = scheduler::decompose(&mut app, &ctx(), id(11), false, Some("heuristic"), None)
        .expect("proposal");
    assert_eq!(dry["task"], "WG-1");
    assert_eq!(dry["created"], json!([]));
    assert_eq!(dry["provider"], "heuristic");
    assert_eq!(dry["model"], "heuristic-v1");
    let children = dry["proposal"]["children"].as_array().expect("children");
    assert!(!children.is_empty());
    assert_eq!(dry["order"].as_array().map(Vec::len), Some(children.len()));
    // A dry run creates nothing.
    assert_eq!(
        app.runtime
            .store
            .read(|s| s.task_world(None, &[]))
            .expect("world")
            .tasks
            .tasks
            .size(),
        2
    );

    let made = scheduler::decompose(&mut app, &ctx(), id(11), true, Some("heuristic"), None)
        .expect("created");
    let keys = made["created"].as_array().expect("keys");
    assert_eq!(keys.len(), children.len());
    let world = app
        .runtime
        .store
        .read(|s| s.task_world(None, &[]))
        .expect("world");
    let kids: Vec<&Task> = world
        .tasks
        .tasks
        .values()
        .filter(|t| t.parent_id == Some(id(11)))
        .collect();
    assert_eq!(kids.len(), children.len());
    // The child that follows another waits for it.
    assert_eq!(
        kids.iter()
            .filter(|t| t.status == TaskStatus::Ready)
            .count(),
        2
    );
    assert_eq!(
        kids.iter()
            .filter(|t| t.status == TaskStatus::Blocked)
            .count(),
        1
    );
    assert!(
        kids.iter()
            .all(|t| world.tasks.tags_of(t.id).any(|tag| tag == "decomposed"))
    );
    let events = app
        .runtime
        .store
        .read(|s| {
            s.query_events(&EventQuery {
                limit: 200,
                ..EventQuery::default()
            })
        })
        .expect("events");
    assert!(events.iter().any(|e| e.event_type == "task.decomposed"));
}

fn proposal(children: Value) -> Result<Proposal, WgError> {
    let data = json!({"children": children, "rationale": "r"});
    read_proposal(data.as_object().expect("object"))
}

#[test]
fn a_decomposition_is_validated_and_ordered() {
    let repos = vec!["api".to_owned()];
    let ordered = proposal(json!([
        {"ref": "c1", "title": "first", "depends_on": ["c2"]},
        {"ref": "c2", "title": "second"},
        {"ref": "c3", "title": "third", "depends_on": ["c1", "c2"], "repositories": ["api"]},
    ]))
    .expect("valid");
    assert_eq!(
        validate_decomposition(&ordered, &repos).expect("order"),
        ["c2", "c1", "c3"]
    );

    let cases = [
        (
            json!([{"ref": "a", "title": "t"}, {"ref": "a", "title": "u"}]),
            ErrorCode::ValidationFailed,
            "decomposition contains duplicate child refs",
        ),
        (
            json!([{"ref": "a", "title": "t", "depends_on": ["z"]}]),
            ErrorCode::ValidationFailed,
            "child a depends on unknown refs ['z']",
        ),
        (
            json!([{"ref": "a", "title": "t", "depends_on": ["a"]}]),
            ErrorCode::DependencyCycle,
            "child a depends on itself",
        ),
        (
            json!([{"ref": "a", "title": "t", "repositories": ["web"]}]),
            ErrorCode::ValidationFailed,
            "child a references repositories outside the parent task: ['web']",
        ),
        (
            json!([
                {"ref": "a", "title": "t", "depends_on": ["b"]},
                {"ref": "b", "title": "u", "depends_on": ["a"]},
            ]),
            ErrorCode::DependencyCycle,
            "proposed child dependencies are cyclic: ['a', 'b', 'a']",
        ),
    ];
    for (children, code, message) in cases {
        let error = validate_decomposition(&proposal(children).expect("shape"), &repos)
            .expect_err("refused");
        assert_eq!(error.code, code);
        assert_eq!(error.message, message);
    }
    let bad = proposal(json!([{"ref": "has space", "title": "t"}])).expect_err("shape");
    assert_eq!(bad.message, "model proposed an invalid decomposition");
}

fn eligible(key: &str, project: Option<u32>, priority: TaskPriority) -> Eligible {
    Eligible {
        task_id: id(100),
        key: key.into(),
        priority,
        project_id: project.map(id),
        agent: None,
        workspace_id: id(0),
    }
}

#[test]
fn the_scheduler_interleaves_projects_by_priority() {
    let order: Vec<String> = fair_order(vec![
        eligible("A1", Some(1), TaskPriority::High),
        eligible("A2", Some(1), TaskPriority::High),
        eligible("B1", Some(2), TaskPriority::Critical),
        eligible("B2", Some(2), TaskPriority::Low),
        eligible("N1", None, TaskPriority::High),
    ])
    .into_iter()
    .map(|item| item.key)
    .collect();
    assert_eq!(order, ["B1", "A1", "N1", "A2", "B2"]);
}

#[test]
fn the_prompt_lists_the_task_the_repositories_and_the_rules() {
    let snapshot = json!({
        "key": "WG-9", "title": "Fix it", "description": "broken",
        "acceptance_criteria": ["works"], "version": 3
    });
    let repos = [
        PromptRepo {
            repository: "api".into(),
            checkout_path: "/w/api".into(),
            access: "read_write".into(),
            sha: Some("0123456789abcdef".into()),
        },
        PromptRepo {
            repository: "docs".into(),
            checkout_path: "/w/docs".into(),
            access: "read_only".into(),
            sha: None,
        },
    ];
    let text = build_prompt(
        snapshot.as_object().expect("object"),
        &repos,
        Some("go slowly"),
    );
    assert!(
        text.starts_with("# Task WG-9: Fix it\n\nYou are working on a task tracked by Workgraph.")
    );
    assert!(text.contains("## Acceptance criteria\n\n- works\n\n## Repositories\n\n"));
    assert!(text.contains("- `api` at `/w/api` (read/write, base 0123456789ab)\n"));
    assert!(text.contains("- `docs` at `/w/docs` (READ-ONLY (do not modify), base None)\n"));
    assert!(text.contains("trailer `Task-ID: WG-9`"));
    assert!(text.contains("(prompt version run-prompt-v1; task version 3)\n\n## Additional instructions from the operator\n\ngo slowly\n"));
}
