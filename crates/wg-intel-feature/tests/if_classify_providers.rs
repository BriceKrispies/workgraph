//! The classification and evaluation services against a fake provider, through the `Providers`
//! trait: an unavailable provider fails the run and stores a failed row, invalid output is stored
//! as a failed row with pydantic's error text, an unsupported capability is stored as an error row,
//! and an evaluation whose provider is unavailable fails. The worker's handler registry holds the
//! feature's job kinds, and the request status is read from the queued job's row.

#![allow(clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rpds::VectorSync;
use serde_json::{Map, Value, json};
use wg_intel_feature::FEATURE;
use wg_intel_feature::jobs::{WithProviders, classify_task_over};
use wg_intel_feature::service::llm::{Provider, Providers};
use wg_intel_feature::service::{classify, ops};
use wg_intel_feature::spec::llm::{CallError, Generation};
use wg_intel_feature::spec::requests::RunOptions;
use wg_jobs::queue::NewJob;
use wg_model::entity::{Classification, ClassificationGoldLabel, Job, Task, Workspace};
use wg_model::{Decision, Write, derive_id};
use wg_netspec::ProviderError;
use wg_runtime::spec::RequestContext;
use wg_runtime::{App, HandlerRegistry, Queue, Worker, WorkerOptions, system_context};
use wg_types::{ErrorCode, Id, JobStatus, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

/// What the fake provider answers every call with.
#[derive(Clone)]
enum Script {
    Answer(Map<String, Value>),
    Unavailable,
    Unsupported(String),
}

/// The `Providers` of a test: every provider name is the same fake. The first
/// `unavailable_calls` calls fail as an unavailable provider, then the script answers; the counter
/// is shared by the clones, so a job's retries see the calls of its earlier attempts.
#[derive(Clone)]
struct Fake {
    script: Script,
    unavailable_calls: Arc<AtomicUsize>,
}

impl Fake {
    fn answering(script: Script) -> Fake {
        Fake {
            script,
            unavailable_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn unavailable_for(calls: usize, script: Script) -> Fake {
        Fake {
            script,
            unavailable_calls: Arc::new(AtomicUsize::new(calls)),
        }
    }
}

impl Providers for Fake {
    fn provider(
        &mut self,
        _name: &str,
        _config: &Map<String, Value>,
    ) -> Result<Box<dyn Provider>, WgError> {
        Ok(Box::new(FakeProvider(self.clone())))
    }
}

struct FakeProvider(Fake);

impl Provider for FakeProvider {
    fn generate_json(
        &mut self,
        _model: &str,
        _system: &str,
        _prompt: &str,
        _schema: &Value,
    ) -> Result<Generation, CallError> {
        let pending =
            self.0
                .unavailable_calls
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                });
        if pending.is_ok() {
            return Err(CallError::Provider(ProviderError::Unavailable(
                "connection refused".to_owned(),
            )));
        }
        match &self.0.script {
            Script::Answer(data) => Ok(Generation {
                data: data.clone(),
                duration_ms: 7,
            }),
            Script::Unavailable => Err(CallError::Provider(ProviderError::Unavailable(
                "connection refused".to_owned(),
            ))),
            Script::Unsupported(message) => Err(CallError::Unsupported(message.clone())),
        }
    }

    fn embed(&mut self, _model: &str, _texts: &[String]) -> Result<Vec<Vec<f64>>, CallError> {
        Err(CallError::Unsupported(
            "the fake provider has no embeddings".to_owned(),
        ))
    }
}

fn valid_answer() -> Map<String, Value> {
    json!({
        "category": "bug_fix",
        "subtype": "regression",
        "domain": "web",
        "affected_systems": ["auth"],
        "complexity": "small",
        "tags": ["login"],
        "confidence": 0.9,
        "rationale": "fixes a crash",
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

fn invalid_answer() -> Map<String, Value> {
    let mut answer = valid_answer();
    answer.insert("category".to_owned(), json!("nope"));
    answer
}

fn at() -> Timestamp {
    Timestamp::parse_iso("2026-01-01T00:00:00Z").expect("a timestamp")
}

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if-providers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// An app with the workspace `acme` (prefix `AC`) and the task `AC-1`. Returns the app, the
/// workspace id and the task id.
fn app_with_task(name: &str) -> (App, Id, Id) {
    let mut app = App::open(Some(&home(name))).expect("the app opens");
    let (workspace_id, task_id) = seed(&mut app);
    (app, workspace_id, task_id)
}

/// Writes the workspace `acme` and the task `AC-1` into the store of `app`. Returns their ids.
fn seed(app: &mut App) -> (Id, Id) {
    let workspace_id = derive_id(&[9; 32], 0);
    let task_id = derive_id(&[9; 32], 1);
    let at = at();
    let workspace = Workspace {
        id: workspace_id,
        slug: "acme".to_owned(),
        name: "Acme".to_owned(),
        description: String::new(),
        key_prefix: "AC".to_owned(),
        next_task_number: 2,
        settings: serde_json::Map::new(),
        archived_at: None,
        created_at: at,
        updated_at: at,
    };
    let task = Task {
        id: task_id,
        workspace_id,
        number: 1,
        key: TaskKey::parse("AC-1").expect("a task key"),
        title: "Fix the login crash".to_owned(),
        description: "The login page crashes on an empty password.".to_owned(),
        description_hash: "h".to_owned(),
        acceptance_criteria: VectorSync::new_sync(),
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
        metadata: serde_json::Map::new(),
        due_at: None,
        scheduled_at: None,
        created_by: "human:brice".to_owned(),
        version: 1,
        created_at: at,
        updated_at: at,
        started_at: None,
        completed_at: None,
        accepted_at: None,
        archived_at: None,
        current_classification_id: None,
    };
    app.runtime
        .store
        .commit_decision(
            &Decision::new(())
                .write(Write::insert(workspace))
                .write(Write::insert(task)),
        )
        .expect("the workspace and the task are written");
    (workspace_id, task_id)
}

fn context() -> RequestContext {
    system_context("classifier").expect("a system context")
}

fn options() -> RunOptions {
    RunOptions {
        provider: None,
        model: None,
        promote: false,
        batch_id: None,
    }
}

fn history(app: &mut App, task_id: Id) -> Vec<Classification> {
    app.runtime
        .store
        .read(|session| classify::classification_history(session, task_id))
        .expect("the history reads")
}

#[test]
fn a_valid_answer_is_stored_as_a_succeeded_row() {
    let (mut app, _, task_id) = app_with_task("valid");
    let row = ops::run_classification(
        &mut app,
        &context(),
        &mut Fake::answering(Script::Answer(valid_answer())),
        task_id,
        &options(),
    )
    .expect("the run succeeds");
    assert_eq!(row.status, "succeeded");
    assert_eq!(row.category.as_deref(), Some("bug_fix"));
}

#[test]
fn an_unavailable_provider_fails_the_run_and_stores_a_failed_row() {
    let (mut app, _, task_id) = app_with_task("unavailable");
    let error = match ops::run_classification(
        &mut app,
        &context(),
        &mut Fake::answering(Script::Unavailable),
        task_id,
        &options(),
    ) {
        Err(error) => error,
        Ok(row) => panic!("the run should fail, not store {row:?}"),
    };
    // The job runner retries a provider-unavailable failure, so the error must be retryable.
    assert_eq!(error.code, ErrorCode::ProviderUnavailable);
    let rows = history(&mut app, task_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "failed");
    assert_eq!(rows[0].validation_status, "provider_unavailable");
    assert_eq!(rows[0].error.as_deref(), Some("connection refused"));
}

#[test]
fn invalid_output_is_stored_as_a_failed_row_with_pydantic_errors() {
    let (mut app, _, task_id) = app_with_task("invalid");
    let row = ops::run_classification(
        &mut app,
        &context(),
        &mut Fake::answering(Script::Answer(invalid_answer())),
        task_id,
        &options(),
    )
    .expect("invalid output is stored, not raised");
    assert_eq!(row.status, "failed");
    assert_eq!(row.validation_status, "invalid_output");
    assert_eq!(row.duration_ms, Some(7));
    assert!(row.raw_output.is_some());
    let error = row.error.unwrap_or_default();
    assert!(
        error.starts_with(
            "output failed schema validation: [{'type': 'literal_error', 'loc': ('category',), 'msg': \"Input should be 'bug_fix', 'feature',"
        ),
        "{error}"
    );
    assert!(error.ends_with("'url': 'https://errors.pydantic.dev/2.14/v/literal_error'}]"));
}

#[test]
fn an_unsupported_capability_is_stored_as_an_error_row() {
    let (mut app, _, task_id) = app_with_task("unsupported");
    let row = ops::run_classification(
        &mut app,
        &context(),
        &mut Fake::answering(Script::Unsupported(
            "heuristic provider cannot produce 'x' outputs".to_owned(),
        )),
        task_id,
        &options(),
    )
    .expect("the capability error is stored, not raised");
    assert_eq!(row.status, "failed");
    assert_eq!(row.validation_status, "error");
    assert_eq!(
        row.error.as_deref(),
        Some("heuristic provider cannot produce 'x' outputs")
    );
}

#[test]
fn an_evaluation_counts_a_failed_label_and_fails_when_the_provider_is_down() {
    let (mut app, workspace_id, task_id) = app_with_task("evaluate");
    let at = at();
    let label = ClassificationGoldLabel {
        id: derive_id(&[9; 32], 2),
        workspace_id,
        task_id: Some(task_id),
        title: "Fix the login crash".to_owned(),
        description: String::new(),
        category: "bug_fix".to_owned(),
        labeled_by: "human:brice".to_owned(),
        created_at: at,
    };
    app.runtime
        .store
        .commit_decision(&Decision::new(()).write(Write::insert(label)))
        .expect("the gold label is written");

    let evaluated = ops::evaluate(
        &mut app,
        &context(),
        &mut Fake::answering(Script::Answer(invalid_answer())),
        Some("acme"),
        None,
        None,
    )
    .expect("a bad answer is a failed sample, not an error");
    assert_eq!(evaluated.row.sample_size, 1);
    assert_eq!(evaluated.row.failures, 1);
    let sample_error = evaluated.results[0]
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        sample_error.starts_with(
            "1 validation error for TaskClassification\ncategory\n  Input should be 'bug_fix', 'feature',"
        ) && sample_error.contains(" [type=literal_error, input_value='nope', input_type=str]\n    For furthe")
            && sample_error.chars().count() == 300,
        "{sample_error}"
    );

    let error = match ops::evaluate(
        &mut app,
        &context(),
        &mut Fake::answering(Script::Unavailable),
        Some("acme"),
        None,
        None,
    ) {
        Err(error) => error,
        Ok(_) => panic!("an unavailable provider fails the evaluation"),
    };
    assert_eq!(error.code, ErrorCode::ProviderUnavailable);
}

#[test]
fn the_request_status_is_the_queued_job_rows_status() {
    let (mut app, _, _) = app_with_task("request");
    let (job_id, _, status) = ops::request(&mut app, &context(), Some("acme"), "AC-1", &options())
        .expect("the request is queued");
    let row = app
        .runtime
        .store
        .read(|session| session.job(job_id))
        .expect("the job reads")
        .expect("the job exists");
    assert_eq!(status, row.status.as_str());
}

#[test]
fn the_worker_registers_the_feature_job_kinds() {
    let registry = HandlerRegistry::from_features(&[&FEATURE]);
    for kind in ["classify_task", "embed_task", "evaluate_classifier"] {
        assert!(
            registry.handlers.contains_key(kind),
            "the worker has no handler for {kind}"
        );
    }
}

/// A worker over `dir` with `registry`, polling quickly.
fn new_worker(dir: &std::path::Path, registry: HandlerRegistry) -> Worker {
    Worker::open(
        WorkerOptions {
            home: Some(dir.to_path_buf()),
            poll_interval: Some(Duration::from_millis(50)),
            maintenance_interval: Some(Duration::from_millis(100)),
            ..WorkerOptions::default()
        },
        registry,
    )
    .expect("the worker opens")
}

/// The feature's registry, with `classify_task` running over `fake` instead of the live providers.
fn registry_over(fake: Fake) -> HandlerRegistry {
    let mut registry = HandlerRegistry::from_features(&[&FEATURE]);
    registry.register(
        "classify_task",
        WithProviders::new(fake, classify_task_over),
    );
    registry
}

/// Queues a `classify_task` job for `task_id` with two attempts and no backoff.
fn enqueue_classify(queue: &mut Queue, workspace_id: Id, task_id: Id) -> Id {
    let payload = json!({"task_id": task_id.to_string()})
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect();
    queue
        .enqueue(&NewJob {
            payload,
            workspace_id: Some(workspace_id),
            task_id: Some(task_id),
            max_attempts: 2,
            backoff_base_seconds: 0.0,
            ..NewJob::new("classify_task")
        })
        .expect("the job is queued")
}

fn job_of(queue: &mut Queue, id: Id) -> Job {
    queue
        .app
        .runtime
        .store
        .read(|session| session.job(id))
        .expect("the job reads")
        .expect("the job exists")
}

#[test]
fn an_unavailable_provider_is_retried_by_the_runner_then_dead_lettered() {
    let dir = home("runner-retry-dead");
    let mut queue = Queue::open(Some(&dir), "test").expect("the queue opens");
    let (workspace_id, task_id) = seed(&mut queue.app);
    let id = enqueue_classify(&mut queue, workspace_id, task_id);
    let mut worker = new_worker(&dir, registry_over(Fake::answering(Script::Unavailable)));
    let summary = worker
        .run_until_idle(Duration::from_secs(30))
        .expect("the worker runs");
    assert_eq!((summary.completed, summary.failed), (0, 2));
    let job = job_of(&mut queue, id);
    assert_eq!(job.status, JobStatus::Dead);
    assert_eq!(job.attempts, 2);
    let rows = history(&mut queue.app, task_id);
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|row| {
            row.status == "failed" && row.validation_status == "provider_unavailable"
        })
    );
}

#[test]
fn an_unavailable_provider_that_recovers_lets_the_retry_succeed() {
    let dir = home("runner-retry-recover");
    let mut queue = Queue::open(Some(&dir), "test").expect("the queue opens");
    let (workspace_id, task_id) = seed(&mut queue.app);
    let id = enqueue_classify(&mut queue, workspace_id, task_id);
    let fake = Fake::unavailable_for(1, Script::Answer(valid_answer()));
    let mut worker = new_worker(&dir, registry_over(fake));
    let summary = worker
        .run_until_idle(Duration::from_secs(30))
        .expect("the worker runs");
    assert_eq!((summary.completed, summary.failed), (1, 1));
    let job = job_of(&mut queue, id);
    assert_eq!(job.status, JobStatus::Succeeded);
    assert_eq!(job.attempts, 2);
    let rows = history(&mut queue.app, task_id);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.iter()
            .filter(|row| row.status == "failed" && row.validation_status == "provider_unavailable")
            .count(),
        1
    );
    assert!(
        rows.iter()
            .any(|row| { row.status == "succeeded" && row.category.as_deref() == Some("bug_fix") })
    );
}
