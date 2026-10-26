//! The classification and evaluation services against a fake provider, through the `Providers`
//! trait: an unavailable provider fails the run and stores a failed row, invalid output is stored
//! as a failed row with pydantic's error text, an unsupported capability is stored as an error row,
//! and an evaluation whose provider is unavailable fails. The worker's handler registry holds the
//! feature's job kinds, and the request status is read from the queued job's row.

#![allow(clippy::expect_used)]

use std::path::PathBuf;

use rpds::VectorSync;
use serde_json::{Map, Value, json};
use wg_intel_feature::FEATURE;
use wg_intel_feature::service::llm::{Provider, Providers};
use wg_intel_feature::service::{classify, ops};
use wg_intel_feature::spec::llm::{CallError, Generation};
use wg_intel_feature::spec::requests::RunOptions;
use wg_model::entity::{Classification, ClassificationGoldLabel, Task, Workspace};
use wg_model::{Decision, Write, derive_id};
use wg_netspec::ProviderError;
use wg_runtime::{App, HandlerRegistry, system_context};
use wg_runtimespec::RequestContext;
use wg_types::{ErrorCode, Id, TaskKey, TaskPriority, TaskStatus, Timestamp, WgError};

/// What the fake provider answers every call with.
#[derive(Clone)]
enum Script {
    Answer(Map<String, Value>),
    Unavailable,
    Unsupported(String),
}

/// The `Providers` of a test: every provider name is the same fake.
struct Fake(Script);

impl Providers for Fake {
    fn provider(
        &mut self,
        _name: &str,
        _config: &Map<String, Value>,
    ) -> Result<Box<dyn Provider>, WgError> {
        Ok(Box::new(FakeProvider(self.0.clone())))
    }
}

struct FakeProvider(Script);

impl Provider for FakeProvider {
    fn generate_json(
        &mut self,
        _model: &str,
        _system: &str,
        _prompt: &str,
        _schema: &Value,
    ) -> Result<Generation, CallError> {
        match &self.0 {
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
    (app, workspace_id, task_id)
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
        &mut Fake(Script::Answer(valid_answer())),
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
        &mut Fake(Script::Unavailable),
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
        &mut Fake(Script::Answer(invalid_answer())),
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
        &mut Fake(Script::Unsupported(
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
        &mut Fake(Script::Answer(invalid_answer())),
        Some("acme"),
        None,
        None,
    )
    .expect("a bad answer is a failed sample, not an error");
    assert_eq!(evaluated.row.sample_size, 1);
    assert_eq!(evaluated.row.failures, 1);

    let error = match ops::evaluate(
        &mut app,
        &context(),
        &mut Fake(Script::Unavailable),
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
