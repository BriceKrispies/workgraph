//! `verify config` and `verify run` (the port of `VerificationService.create_config` and `run`):
//! scope resolution, the config events, the check runner over scripted checks, the run rows and
//! events, the attempt and task moves, and the automatic acceptance.
//!
//! Each test opens an application on a fresh home. The checks are small script files run through
//! the platform shell (`allow_shell`), so no real test suite is involved.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use rpds::VectorSync;
use serde_json::{Value as Json, json};
use wg_iface::cli::args::{get, items, python};
use wg_json::Value;
use wg_model::entity::{
    AgentAttempt, Project, Repository, RepositoryCheckout, Task, TaskRepository, Workspace,
};
use wg_model::{JsonObject, Row, Write, derive_id};
use wg_runtime::{App, RequestContext, human_context};
use wg_store::EventQuery;
use wg_types::{
    AccessMode, AttemptStatus, Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError,
};
use wg_verify::CheckTier;
use wg_verify_feature::service::config::config;
use wg_verify_feature::service::run::{RunInput, run};
use wg_verify_feature::spec::config_write::ConfigRequest;

type TestResult = Result<(), Box<dyn Error>>;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn id(n: u32) -> Id {
    derive_id(&[7; 32], n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 26, 12, 0, second, 0))
}

fn obj(value: Json) -> JsonObject {
    match value {
        Json::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

/// An application on a fresh home, with one workspace, one project, one repository whose
/// checkout is a directory under the home, and one task awaiting verification in it.
struct Fixture {
    app: App,
    home: PathBuf,
    context: RequestContext,
}

impl Fixture {
    fn open(status: TaskStatus) -> Result<Fixture, Box<dyn Error>> {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("wg3855-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("checkout"))?;
        let mut app = App::open(Some(&home))?;
        let rows: Vec<Row> = vec![
            workspace().into(),
            project().into(),
            repository().into(),
            RepositoryCheckout {
                id: id(4),
                repository_id: id(3),
                path: home.join("checkout").display().to_string(),
                kind: "clone".into(),
                is_primary: true,
                access_mode: None,
                available: true,
                last_seen_at: None,
                created_at: at(0),
                updated_at: at(0),
            }
            .into(),
            task(status).into(),
            TaskRepository {
                task_id: id(2),
                repository_id: id(3),
                access: "read_write".into(),
                created_at: at(1),
            }
            .into(),
        ];
        app.runtime.store.write(|session| {
            rows.into_iter()
                .try_for_each(|row| session.apply_write(&Write::insert(row)))
        })?;
        Ok(Fixture {
            app,
            home,
            context: human_context(Some("tester"))?,
        })
    }

    fn config(&mut self, name: &str, checks: &[Json]) -> Result<Value, WgError> {
        let request = ConfigRequest {
            scope_type: "workspace",
            scope: None,
            name,
            checks,
        };
        config(&mut self.app, &self.context, Some("ws"), &request)
    }

    fn run(&mut self, attempt: Option<&str>, tier: Option<CheckTier>) -> Result<Value, WgError> {
        let input = RunInput {
            workspace: Some("ws"),
            task: "WG-7",
            attempt,
            tier,
        };
        run(&mut self.app, &self.context, &input, None)
    }

    fn event_types(&mut self) -> Result<Vec<String>, Box<dyn Error>> {
        let events = self.app.runtime.store.read(|session| {
            session.query_events(&EventQuery {
                limit: 1000,
                ..EventQuery::default()
            })
        })?;
        Ok(events.into_iter().map(|event| event.event_type).collect())
    }

    fn task_status(&mut self) -> Result<TaskStatus, Box<dyn Error>> {
        let world = self
            .app
            .runtime
            .store
            .read(|session| session.task_world(None, &[]))?;
        Ok(world
            .tasks
            .task(id(2))
            .map(|found| found.status)
            .ok_or("task")?)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn workspace() -> Workspace {
    Workspace {
        id: id(1),
        slug: "ws".into(),
        name: "ws".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 10,
        settings: obj(json!({})),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

fn project() -> Project {
    Project {
        id: id(5),
        workspace_id: id(1),
        slug: "core".into(),
        name: "core".into(),
        description: String::new(),
        status: "active".into(),
        settings: obj(json!({})),
        created_at: at(0),
        updated_at: at(0),
    }
}

fn repository() -> Repository {
    Repository {
        id: id(3),
        workspace_id: id(1),
        name: "app".into(),
        description: String::new(),
        identity_fingerprint: None,
        default_branch: None,
        tracked_branches: VectorSync::new_sync(),
        access_mode: AccessMode::ReadWrite,
        health_status: "ok".into(),
        health_detail: obj(json!({})),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: None,
        metadata: obj(json!({})),
        created_at: at(0),
        updated_at: at(0),
    }
}

fn task(status: TaskStatus) -> Task {
    Task {
        id: id(2),
        workspace_id: id(1),
        number: 7,
        key: ok(TaskKey::parse("WG-7")),
        title: "verified task".into(),
        description: String::new(),
        description_hash: "h".into(),
        acceptance_criteria: VectorSync::new_sync(),
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
        metadata: obj(json!({})),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".into(),
        version: 1,
        created_at: at(1),
        updated_at: at(1),
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    }
}

fn attempt(status: AttemptStatus) -> AgentAttempt {
    AgentAttempt {
        id: id(6),
        workspace_id: id(1),
        task_id: id(2),
        number: 1,
        status,
        provider: "claude".into(),
        adapter: "claude".into(),
        agent_id: None,
        model: None,
        session_id: None,
        parent_attempt_id: None,
        retry_of_attempt_id: None,
        rerun_of_attempt_id: None,
        created_by: "human:brice".into(),
        created_at: at(2),
        queued_at: None,
        started_at: Some(at(3)),
        finished_at: None,
        queue_wait_ms: None,
        duration_ms: None,
        exit_reason: None,
        exit_code: None,
        error: None,
        summary: None,
        input_task_version: None,
        input_task_snapshot: JsonObject::new(),
        prompt_artifact_id: None,
        config_snapshot: JsonObject::new(),
        environment: JsonObject::new(),
        baseline_manifest_id: None,
        outcome_manifest_id: None,
        usage: JsonObject::new(),
        usage_source: None,
        cost_usd: None,
        cost_source: None,
        telemetry: JsonObject::new(),
        modified_repository_ids: VectorSync::new_sync(),
        retries: 0,
        cancel_requested: false,
        updated_at: at(3),
    }
}

/// A check that runs a script: `windows` for `cmd`, `unix` for `sh`.
fn scripted(dir: &Path, name: &str, windows: &str, unix: &str) -> Result<Json, Box<dyn Error>> {
    let (shell, flag, file, body) = if cfg!(windows) {
        (
            "cmd",
            "/c",
            dir.join(format!("{name}.cmd")),
            format!("@echo off\r\n{windows}\r\n"),
        )
    } else {
        (
            "sh",
            "-e",
            dir.join(format!("{name}.sh")),
            format!("{unix}\n"),
        )
    };
    std::fs::write(&file, body)?;
    Ok(json!({
        "name": name,
        "command": [shell, flag, file.display().to_string()],
        "allow_shell": true,
        "parser": "pytest",
    }))
}

fn passing(dir: &Path, name: &str) -> Result<Json, Box<dyn Error>> {
    scripted(dir, name, "echo all good\r\nexit /b 0", "echo all good")
}

fn failing(dir: &Path, name: &str) -> Result<Json, Box<dyn Error>> {
    scripted(
        dir,
        name,
        "echo FAILED tests/t.py::test_one\r\nexit /b 1",
        "echo 'FAILED tests/t.py::test_one'\nexit 1",
    )
}

fn text(value: &Value, key: &str) -> String {
    get(value, key).map(python).unwrap_or_default()
}

fn check_named<'a>(out: &'a Value, name: &str) -> Result<&'a Value, Box<dyn Error>> {
    items(get(out, "checks"))
        .iter()
        .find(|check| text(check, "name") == name)
        .ok_or_else(|| format!("no check named {name}").into())
}

fn position(types: &[String], event: &str) -> Result<usize, Box<dyn Error>> {
    types
        .iter()
        .position(|found| found == event)
        .ok_or_else(|| format!("no {event} event in {types:?}").into())
}

// covers: C172
#[test]
fn config_creates_then_updates_one_scope_and_name() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let dir = fx.home.clone();
    let first = fx.config("default", &[passing(&dir, "unit")?])?;
    assert_eq!(text(&first, "version"), "1");
    assert_eq!(text(&first, "scope_type"), "workspace");
    assert_eq!(text(&first, "scope_id"), id(1).to_string());
    assert_eq!(items(get(&first, "checks")).len(), 1);

    let second = fx.config("default", &[passing(&dir, "unit")?, failing(&dir, "lint")?])?;
    assert_eq!(
        text(&second, "id"),
        text(&first, "id"),
        "the same row is updated"
    );
    assert_eq!(text(&second, "version"), "2");
    assert_eq!(items(get(&second, "checks")).len(), 2);

    let types = fx.event_types()?;
    let created = position(&types, "verification.config_created")?;
    let updated = position(&types, "verification.config_updated")?;
    assert!(created < updated);
    Ok(())
}

// covers: C172
#[test]
fn config_resolves_project_and_repository_scopes_through_the_registry() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let checks = [passing(&fx.home.clone(), "unit")?];
    for (scope_type, scope, expected) in [
        ("project", "core", id(5)),
        ("repository", "app", id(3)),
        ("task", "WG-7", id(2)),
    ] {
        let request = ConfigRequest {
            scope_type,
            scope: Some(scope),
            name: scope_type,
            checks: &checks,
        };
        let out = config(&mut fx.app, &fx.context, Some("ws"), &request)?;
        assert_eq!(text(&out, "scope_id"), expected.to_string(), "{scope_type}");
    }
    Ok(())
}

// covers: C172
#[test]
fn config_refuses_what_python_refuses() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let checks = [passing(&fx.home.clone(), "unit")?];
    let ask = |fx: &mut Fixture, scope_type: &str, scope: Option<&str>, checks: &[Json]| {
        let request = ConfigRequest {
            scope_type,
            scope,
            name: "n",
            checks,
        };
        config(&mut fx.app, &fx.context, Some("ws"), &request)
    };
    let missing = ask(&mut fx, "project", None, &checks);
    assert_eq!(
        missing.err().map(|e| e.message),
        Some("scope reference required for scope type project".to_owned())
    );
    let unknown = ask(&mut fx, "galaxy", Some("x"), &checks);
    assert_eq!(
        unknown.err().map(|e| e.message),
        Some("unknown scope type 'galaxy'".to_owned())
    );
    let empty = ask(&mut fx, "workspace", None, &[]);
    assert_eq!(
        empty.err().map(|e| e.message),
        Some("a verification configuration needs at least one check".to_owned())
    );
    let twin = ask(
        &mut fx,
        "workspace",
        None,
        &[checks[0].clone(), checks[0].clone()],
    );
    assert_eq!(
        twin.err().map(|e| e.message),
        Some("check names must be unique within a configuration".to_owned())
    );
    let shell = json!({"name": "s", "command": ["bash", "-c", "true"]});
    let refused = ask(&mut fx, "workspace", None, &[shell]);
    assert!(
        refused
            .err()
            .is_some_and(|e| e.message.contains("allow_shell"))
    );
    let nowhere = json!({"name": "r", "command": ["true"], "repository": "ghost"});
    let absent = ask(&mut fx, "workspace", None, &[nowhere]);
    assert!(
        absent
            .err()
            .is_some_and(|e| e.message.contains("repository 'ghost' not found"))
    );
    Ok(())
}

// covers: C173
#[test]
fn a_failing_required_check_fails_the_run_and_the_task() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let dir = fx.home.clone();
    fx.config("default", &[passing(&dir, "unit")?, failing(&dir, "lint")?])?;
    let out = fx.run(None, None)?;
    assert_eq!(text(&out, "status"), "failed");
    assert_eq!(text(&out, "required_passed"), "False");
    let names: Vec<String> = items(get(&out, "checks"))
        .iter()
        .map(|c| text(c, "name"))
        .collect();
    assert_eq!(
        names,
        ["unit", "lint"],
        "results keep the order the checks ran in"
    );
    let lint = check_named(&out, "lint")?;
    assert_eq!(text(lint, "status"), "failed");
    assert_eq!(text(lint, "exit_code"), "1");
    assert_eq!(
        items(get(lint, "failure_signatures"))
            .iter()
            .map(python)
            .collect::<Vec<_>>(),
        ["test:tests/t.py::test_one"]
    );
    assert!(text(lint, "output_excerpt").contains("FAILED tests/t.py::test_one"));
    assert!(
        !text(lint, "log_artifact_id").is_empty(),
        "the log is registered"
    );
    assert_eq!(text(check_named(&out, "unit")?, "status"), "passed");

    let manifest = items(get(&out, "manifest"));
    assert_eq!(manifest.len(), 1);
    assert_eq!(text(&manifest[0], "repository"), "app");
    assert_eq!(text(&manifest[0], "access"), "read_write");

    assert_eq!(fx.task_status()?, TaskStatus::Failed);
    let types = fx.event_types()?;
    let started = position(&types, "verification.started")?;
    let completed = position(&types, "verification.completed")?;
    assert!(started < completed);
    assert!(
        types[completed..]
            .iter()
            .any(|t| t == "task.status_changed")
    );
    assert!(!types.iter().any(|t| t == "task.accepted"));
    Ok(())
}

// covers: C173
#[test]
fn a_passing_full_run_accepts_the_task_automatically() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let dir = fx.home.clone();
    fx.config("default", &[passing(&dir, "unit")?])?;
    let out = fx.run(None, None)?;
    assert_eq!(text(&out, "status"), "passed");
    assert_eq!(text(&out, "required_passed"), "True");
    assert_eq!(fx.task_status()?, TaskStatus::Accepted);

    let acceptances = fx.app.runtime.store.read(|s| s.task_acceptances(id(2)))?;
    assert_eq!(acceptances.len(), 1);
    assert_eq!(acceptances[0].mode, "automated");
    assert_eq!(acceptances[0].reason, "verification.auto_accept");
    assert_eq!(
        acceptances[0].verification_run_id.map(|r| r.to_string()),
        Some(text(&out, "id"))
    );
    assert!(
        acceptances[0].manifest_id.is_some(),
        "the accepted manifest is kept"
    );
    let types = fx.event_types()?;
    assert!(position(&types, "verification.completed")? < position(&types, "task.accepted")?);
    Ok(())
}

// covers: C173
#[test]
fn a_fast_tier_pass_leaves_the_task_where_it_was() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let dir = fx.home.clone();
    let mut lint = passing(&dir, "lint")?;
    lint["tier"] = json!("fast");
    fx.config("default", &[lint, passing(&dir, "unit")?])?;
    let out = fx.run(None, Some(CheckTier::Fast))?;
    assert_eq!(text(&out, "status"), "passed");
    assert_eq!(
        items(get(&out, "checks")).len(),
        1,
        "only the fast check ran"
    );
    assert_eq!(fx.task_status()?, TaskStatus::AwaitingVerification);
    Ok(())
}

// covers: C173
#[test]
fn a_run_with_no_checks_in_the_tier_is_refused() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let none = fx.run(None, None).err().map(|e| e.message);
    assert_eq!(
        none,
        Some(
            "no verification checks are configured for WG-7; record manual acceptance instead"
                .to_owned()
        )
    );
    let dir = fx.home.clone();
    fx.config("default", &[passing(&dir, "unit")?])?;
    let fast = fx.run(None, Some(CheckTier::Fast)).err().map(|e| e.message);
    assert_eq!(
        fast,
        Some("no fast verification checks are configured for WG-7".to_owned())
    );
    Ok(())
}

// covers: C173
#[test]
fn checks_that_cannot_run_are_errors_with_signatures() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let dir = fx.home.clone();
    let ghost = json!({"name": "ghost", "command": ["wg3855-no-such-program"]});
    fx.config("default", &[ghost, passing(&dir, "unit")?])?;
    let out = fx.run(None, None)?;
    let ghost = check_named(&out, "ghost")?;
    assert_eq!(text(ghost, "status"), "error");
    assert_eq!(
        text(ghost, "output_excerpt"),
        "executable 'wg3855-no-such-program' was not found on PATH"
    );
    assert_eq!(
        items(get(ghost, "failure_signatures"))
            .iter()
            .map(python)
            .collect::<Vec<_>>(),
        ["error:executable 'wg3855-no-such-program' was not found on PATH"]
    );
    assert_eq!(text(&out, "status"), "failed");
    Ok(())
}

// covers: C173
#[test]
fn a_timed_out_check_leads_with_the_timeout_signature() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let dir = fx.home.clone();
    let mut slow = scripted(&dir, "slow", "ping -n 30 127.0.0.1 >nul", "sleep 30")?;
    slow["timeout_seconds"] = json!(0.5);
    fx.config("default", &[slow])?;
    let out = fx.run(None, None)?;
    let slow = check_named(&out, "slow")?;
    assert_eq!(text(slow, "timed_out"), "True");
    assert_eq!(text(slow, "status"), "failed");
    let signatures: Vec<String> = items(get(slow, "failure_signatures"))
        .iter()
        .map(python)
        .collect();
    assert_eq!(signatures.first().map(String::as_str), Some("timeout"));
    Ok(())
}

// covers: C173
#[test]
fn the_attempt_goes_through_verifying_and_finishes_with_the_verdict() -> TestResult {
    let mut fx = Fixture::open(TaskStatus::AwaitingVerification)?;
    let dir = fx.home.clone();
    fx.app
        .runtime
        .store
        .write(|session| session.apply_write(&Write::insert(attempt(AttemptStatus::Running))))?;
    fx.config("default", &[failing(&dir, "lint")?])?;
    let out = fx.run(Some(&id(6).to_string()), None)?;
    assert_eq!(text(&out, "attempt_id"), id(6).to_string());
    let finished = fx
        .app
        .runtime
        .store
        .read(|session| session.attempt_by_id(id(6)))?
        .ok_or("attempt")?;
    assert_eq!(finished.status, AttemptStatus::Failed);
    assert_eq!(finished.exit_reason.as_deref(), Some("verification_failed"));
    assert_eq!(finished.error.as_deref(), Some("verification failed: lint"));
    let types = fx.event_types()?;
    let moved = position(&types, "attempt.status_changed")?;
    let started = position(&types, "verification.started")?;
    let completed = position(&types, "verification.completed")?;
    let failed = position(&types, "attempt.failed")?;
    assert!(moved < started && started < completed && completed < failed);

    let unknown = fx.run(Some("00000000-0000-4000-8000-000000000000"), None);
    assert!(
        unknown
            .err()
            .is_some_and(|e| e.message.contains("not found"))
    );
    Ok(())
}
