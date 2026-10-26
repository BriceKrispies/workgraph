//! The task group (IF-44): the routes H023 to H046 without the acceptance routes, replayed
//! in-process against the router with the group registered.
//!
//! `if44/scenario.json` is the list of requests; `if44/golden.json` is what the Python app
//! answered to them (Starlette's `TestClient` on `create_app`, over a home seeded with the same
//! workspace, project and repositories). The replay normalises ids and clocks on both sides and
//! compares status and body one request at a time.

use std::collections::BTreeMap;
use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::Request;
use serde_json::{Map, Value, json};
use tower::ServiceExt;
use wg_model::entity::{Project, Repository, Workspace};
use wg_model::event::{AttemptCreated, EventDraft, EventPayload};
use wg_model::{Decision, Write};
use wg_runtime::{App, RequestContext};
use wg_server::{Server, ServerOptions, Ui, groups, router};
use wg_types::{AccessMode, AttemptStatus, Id, TaskKey, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// The port the server believes it is bound to (the allowed hosts name it). Nothing binds it.
const PORT: u16 = 8765;

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if44")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn here(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("if44")
        .join(file)
}

fn read_json(file: &str) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(here(file))?)?)
}

fn seed_id(slot: u8) -> Id {
    wg_model::derive_id(&[slot; 32], 0)
}

/// The workspace, project and repositories the golden run was seeded with.
fn seed(app: &mut App) -> Result<(), Box<dyn Error>> {
    let at = wg_types::Timestamp::parse_iso("2026-01-01T00:00:00Z")?;
    let workspace = Workspace {
        id: seed_id(1),
        slug: "main".to_owned(),
        name: "Main".to_owned(),
        description: String::new(),
        key_prefix: "WG".to_owned(),
        next_task_number: 1,
        settings: Map::new(),
        archived_at: None,
        created_at: at,
        updated_at: at,
    };
    let project = Project {
        id: seed_id(2),
        workspace_id: workspace.id,
        slug: "proj".to_owned(),
        name: "Proj".to_owned(),
        description: String::new(),
        status: "active".to_owned(),
        settings: Map::new(),
        created_at: at,
        updated_at: at,
    };
    let repository = |slot: u8, name: &str, access_mode: AccessMode| Repository {
        id: seed_id(slot),
        workspace_id: workspace.id,
        name: name.to_owned(),
        description: String::new(),
        identity_fingerprint: None,
        default_branch: Some("main".to_owned()),
        tracked_branches: Default::default(),
        access_mode,
        health_status: "ok".to_owned(),
        health_detail: Map::new(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: None,
        metadata: Map::new(),
        created_at: at,
        updated_at: at,
    };
    app.runtime.store.write(|session| {
        session.apply_write(&Write::insert(workspace.clone()))?;
        session.apply_write(&Write::insert(project))?;
        session.apply_write(&Write::insert(repository(3, "web", AccessMode::ReadWrite)))?;
        session.apply_write(&Write::insert(repository(4, "ro", AccessMode::ReadOnly)))
    })?;
    let mut config = app.config()?;
    let llm = config
        .entry("llm")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("llm is not a table")?;
    llm.insert("auto_classify".to_owned(), json!(false));
    llm.insert("auto_embed".to_owned(), json!(false));
    app.save_config(&config)?;
    Ok(())
}

/// `app.attempts.create`: a running external attempt of the task `key`, with its event.
fn create_attempt(app: &mut App, ctx: &RequestContext, key: &str) -> Result<Id, WgError> {
    let key = TaskKey::parse(key)?;
    let outcome = app.runtime.decide(
        ctx,
        |session| session.task_world(None, &[]),
        |world, stamp| {
            let task = world
                .tasks
                .tasks
                .values()
                .find(|task| task.key == key)
                .ok_or_else(|| WgError::new(wg_types::ErrorCode::NotFound, "no such task"))?;
            let attempt_id = stamp.id(100);
            let attempt = wg_model::entity::AgentAttempt {
                id: attempt_id,
                workspace_id: task.workspace_id,
                task_id: task.id,
                number: 1,
                status: AttemptStatus::Running,
                provider: "external".to_owned(),
                adapter: "external".to_owned(),
                agent_id: Some(stamp.actor.to_string()),
                model: None,
                session_id: None,
                parent_attempt_id: None,
                retry_of_attempt_id: None,
                rerun_of_attempt_id: None,
                created_by: stamp.actor.to_string(),
                created_at: stamp.at,
                queued_at: Some(stamp.at),
                started_at: Some(stamp.at),
                finished_at: None,
                queue_wait_ms: Some(0),
                duration_ms: None,
                exit_reason: None,
                exit_code: None,
                error: None,
                summary: None,
                input_task_version: Some(task.version),
                input_task_snapshot: Map::new(),
                prompt_artifact_id: None,
                config_snapshot: Map::new(),
                environment: Map::new(),
                baseline_manifest_id: None,
                outcome_manifest_id: None,
                usage: Map::new(),
                usage_source: None,
                cost_usd: None,
                cost_source: None,
                telemetry: Map::new(),
                modified_repository_ids: Default::default(),
                retries: 0,
                cancel_requested: false,
                updated_at: stamp.at,
            };
            let event = EventDraft::new(
                stamp,
                stamp.id(101),
                Some(task.workspace_id),
                attempt_id,
                EventPayload::AttemptCreated(AttemptCreated {
                    task_id: task.id,
                    task_key: task.key.clone(),
                    number: 1,
                    provider: "external".to_owned(),
                    adapter: "external".to_owned(),
                    model: None,
                    status: AttemptStatus::Running,
                    parent_attempt_id: None,
                    retry_of_attempt_id: None,
                    rerun_of_attempt_id: None,
                }),
            )
            .with_attempt(Some(attempt_id));
            Ok(Decision::new(attempt_id)
                .write(Write::insert(attempt))
                .event(event))
        },
    )?;
    Ok(outcome.reply)
}

/// `create_config` for a workspace-wide gate named `gate` with one check.
fn create_gate(app: &mut App, ctx: &RequestContext) -> Result<(), WgError> {
    let workspace = app.workspace_id("main")?;
    app.runtime.decide(
        ctx,
        |_| Ok(()),
        |_, stamp| {
            let check = json!({"name": "unit", "command": ["true"]});
            let config = wg_model::entity::VerificationConfig {
                id: stamp.id(200),
                workspace_id: workspace,
                scope_type: "workspace".to_owned(),
                scope_id: workspace,
                name: "gate".to_owned(),
                checks: std::iter::once(check.as_object().cloned().unwrap_or_default()).collect(),
                enabled: true,
                approved_by: stamp.actor.to_string(),
                version: 1,
                created_at: stamp.at,
                updated_at: stamp.at,
            };
            Ok(Decision::new(()).write(Write::insert(config)))
        },
    )?;
    Ok(())
}

/// One server with the group registered over a seeded home, and the API token.
struct Harness {
    server: Arc<Server>,
    token: String,
    ctx: RequestContext,
}

/// A recorded exchange.
#[derive(Debug, Clone)]
struct Exchange {
    name: String,
    status: u64,
    body: Value,
}

impl Harness {
    fn new(name: &str) -> Result<Harness, Box<dyn Error>> {
        let mut app = App::open(Some(&scratch(name)?.join("home")))?;
        seed(&mut app)?;
        let ctx = RequestContext::human(Some("tester"), None, wg_model::derive_id(&[9; 32], 0))?;
        let ui = scratch(&format!("{name}-ui"))?;
        let server = Server::new(
            ServerOptions {
                host: "127.0.0.1".to_owned(),
                port: PORT,
                lan: None,
                ui: Ui::Dir(ui),
                mcp: None,
                routes: groups::routes_with(&[]),
            },
            app,
        )?;
        let token = server.policy.token.clone();
        Ok(Harness {
            server: Arc::new(server),
            token,
            ctx,
        })
    }

    /// Sends one request as the local operator `tester`; the body of an error that is not JSON
    /// is kept as `{"_text": ...}`.
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), Box<dyn Error>> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("authorization", format!("Bearer {}", self.token))
            .header("x-workgraph-actor", "tester");
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let payload = body.map_or_else(Body::empty, |value| Body::from(value.to_string()));
        let mut request = builder.body(payload)?;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
        let response = router(Arc::clone(&self.server)).oneshot(request).await?;
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, usize::MAX).await?;
        let text = String::from_utf8(bytes.to_vec())?;
        let value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"_text": text}));
        Ok((parts.status.as_u16(), value))
    }

    /// Runs the scenario in order and records every response.
    async fn replay(&self, scenario: &[Value]) -> Result<Vec<Exchange>, Box<dyn Error>> {
        let mut bound: BTreeMap<String, String> = BTreeMap::new();
        let mut log: Vec<Exchange> = Vec::new();
        for step in scenario {
            if step["hook"] == json!("verification") {
                let ctx = self.ctx.clone();
                self.server
                    .blocking(move |app| create_gate(app, &ctx))
                    .await?;
                continue;
            }
            if step["hook"] == json!("attempt") {
                let key = step["task"].as_str().ok_or("hook without a task")?;
                let name = step["as"].as_str().ok_or("hook without a name")?;
                let id = {
                    let ctx = self.ctx.clone();
                    let key = key.to_owned();
                    self.server
                        .blocking(move |app| create_attempt(app, &ctx, &key))
                        .await?
                };
                bound.insert(name.to_owned(), id.to_string());
                continue;
            }
            let path = step["path"].as_str().ok_or("step without a path")?;
            let mut url = if path.starts_with("/api/") {
                path.to_owned()
            } else {
                format!("/api/v1/ws/main{path}")
            };
            if let Some(pairs) = step["query"].as_array() {
                let query: Vec<String> = pairs
                    .iter()
                    .map(|pair| {
                        format!(
                            "{}={}",
                            encode(pair[0].as_str().unwrap_or_default()),
                            encode(&fill(pair[1].as_str().unwrap_or_default(), &bound))
                        )
                    })
                    .collect();
                url = format!("{url}?{}", query.join("&"));
            }
            let body = step.get("body").map(|value| fill_json(value, &bound));
            let method = step["method"].as_str().ok_or("step without a method")?;
            let (status, body) = self.send(method, &url, body.as_ref()).await?;
            log.push(Exchange {
                name: step["name"].as_str().unwrap_or_default().to_owned(),
                status: u64::from(status),
                body,
            });
        }
        Ok(log)
    }
}

/// Percent-encodes a query component.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Replaces `{name}` with the value bound to it.
fn fill(text: &str, bound: &BTreeMap<String, String>) -> String {
    bound.iter().fold(text.to_owned(), |acc, (name, value)| {
        acc.replace(&format!("{{{name}}}"), value)
    })
}

fn fill_json(value: &Value, bound: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(text) => Value::String(fill(text, bound)),
        Value::Array(items) => Value::Array(items.iter().map(|v| fill_json(v, bound)).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), fill_json(v, bound)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(at, c)| match at {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// An ISO timestamp, as `Z`, `+00:00` or naive text; the suffix tells which.
fn timestamp_suffix(text: &str) -> Option<&'static str> {
    let digits = |from: usize, to: usize| {
        text.get(from..to)
            .is_some_and(|part| part.bytes().all(|b| b.is_ascii_digit()))
    };
    let shape = text.len() >= 19
        && digits(0, 4)
        && digits(5, 7)
        && digits(8, 10)
        && digits(11, 13)
        && digits(14, 16)
        && digits(17, 19)
        && matches!(text.as_bytes().get(10), Some(b'T' | b' '));
    if !shape {
        return None;
    }
    let rest = text[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    match rest {
        "Z" => Some("Z"),
        "+00:00" => Some("+00:00"),
        "" => Some(""),
        _ => None,
    }
}

fn is_task_key(text: &str) -> bool {
    text.split_once('-').is_some_and(|(prefix, number)| {
        !prefix.is_empty()
            && prefix.bytes().all(|b| b.is_ascii_uppercase())
            && !number.is_empty()
            && number.bytes().all(|b| b.is_ascii_digit())
    })
}

/// The ids of the tasks in a document, with their keys: every object with both.
fn task_ids(value: &Value, found: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(fields) => {
            if let (Some(Value::String(id)), Some(Value::String(key))) =
                (fields.get("id"), fields.get("key"))
                && is_uuid(id)
                && is_task_key(key)
            {
                found.insert(id.clone(), key.clone());
            }
            fields.values().for_each(|v| task_ids(v, found));
        }
        Value::Array(items) => items.iter().for_each(|v| task_ids(v, found)),
        _ => {}
    }
}

/// The document with its volatile parts replaced: task ids by their keys, other ids by their
/// order of appearance, clocks by their format and event sequence numbers by a mark.
fn normalise(value: &Value, tasks: &BTreeMap<String, String>, seen: &mut Vec<String>) -> Value {
    match value {
        Value::String(text) if is_uuid(text) => match tasks.get(text) {
            Some(key) => json!(format!("<task {key}>")),
            None => {
                let at = seen
                    .iter()
                    .position(|known| known == text)
                    .unwrap_or_else(|| {
                        seen.push(text.clone());
                        seen.len() - 1
                    });
                json!(format!("<id {at}>"))
            }
        },
        Value::String(text) => match timestamp_suffix(text) {
            Some(suffix) => json!(format!("<time{suffix}>")),
            None => value.clone(),
        },
        Value::Array(items) => {
            let done: Vec<Value> = items.iter().map(|v| normalise(v, tasks, seen)).collect();
            // A list of tasks is in the order of their random ids: compare it as a set.
            let tasks_only = !done.is_empty()
                && done
                    .iter()
                    .all(|v| v.as_str().is_some_and(|text| text.starts_with("<task ")));
            if tasks_only {
                let mut sorted = done;
                sorted.sort_by_key(Value::to_string);
                Value::Array(sorted)
            } else {
                Value::Array(done)
            }
        }
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, v)| {
                    let done = if key == "seq" || key == "duration_ms" {
                        json!("<seq>")
                    } else {
                        normalise(v, tasks, seen)
                    };
                    (key.clone(), done)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Every exchange of a log, normalised against the task ids the whole log shows.
fn normal(log: &[Value]) -> Vec<Value> {
    let mut tasks = BTreeMap::new();
    log.iter()
        .for_each(|exchange| task_ids(exchange, &mut tasks));
    let mut seen = Vec::new();
    log.iter()
        .map(|exchange| normalise(exchange, &tasks, &mut seen))
        .collect()
}

/// The golden log as a list of `{name, status, body}` values.
fn golden() -> Result<Vec<Value>, Box<dyn Error>> {
    match read_json("golden.json")? {
        Value::Array(list) => Ok(list),
        _ => Err("the golden log is not a list".into()),
    }
}

fn scenario() -> Result<Vec<Value>, Box<dyn Error>> {
    match read_json("scenario.json")? {
        Value::Array(list) => Ok(list),
        _ => Err("the scenario is not a list".into()),
    }
}

/// The first difference between two normalised values, as a path and the two texts.
fn first_difference(path: &str, left: &Value, right: &Value) -> Option<String> {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            let missing = a.keys().find(|k| !b.contains_key(*k));
            let extra = b.keys().find(|k| !a.contains_key(*k));
            if let Some(key) = missing {
                return Some(format!("{path}.{key}: golden has it, replay does not"));
            }
            if let Some(key) = extra {
                return Some(format!("{path}.{key}: replay has it, golden does not"));
            }
            a.iter()
                .find_map(|(key, v)| first_difference(&format!("{path}.{key}"), v, &b[key]))
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!(
                    "{path}: golden has {} items, replay {}",
                    a.len(),
                    b.len()
                ));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(at, (x, y))| first_difference(&format!("{path}[{at}]"), x, y))
        }
        (a, b) if a == b => None,
        (a, b) => Some(format!("{path}: golden {a} against replay {b}")),
    }
}

// covers: H023, H024, H025, H026, H027, H028, H029, H030, H031, H032, H036, H037, H038, H039, H040, H041, H042, H043, H044, H045, H046
#[tokio::test]
async fn every_route_answers_as_the_python_app_did() -> TestResult {
    let scenario = scenario()?;
    let golden = golden()?;
    let harness = Harness::new("replay")?;
    let replayed = harness.replay(&scenario).await?;
    if let Ok(path) = std::env::var("WG_IF44_DUMP") {
        let dump: Vec<Value> = replayed
            .iter()
            .map(|e| json!({"name": e.name, "status": e.status, "body": e.body}))
            .collect();
        std::fs::write(path, serde_json::to_string_pretty(&dump)?)?;
    }
    assert_eq!(replayed.len(), golden.len(), "one answer per request");
    let mine: Vec<Value> = replayed
        .iter()
        .map(|e| json!({"name": e.name, "status": e.status, "body": e.body}))
        .collect();
    let differences: Vec<String> = normal(&mine)
        .iter()
        .zip(normal(&golden).iter())
        .filter_map(|(mine, theirs)| {
            first_difference("", theirs, mine)
                .map(|found| format!("{}: {found}", mine["name"].as_str().unwrap_or_default()))
        })
        .collect();
    assert!(
        differences.is_empty(),
        "{} of {} answers differ:\n{}",
        differences.len(),
        golden.len(),
        differences.join("\n")
    );
    Ok(())
}

fn actor(
    kind: &str,
    permissions: Option<std::collections::BTreeSet<wg_types::Permission>>,
) -> Result<RequestContext, Box<dyn Error>> {
    let correlation = wg_model::derive_id(&[8; 32], 0);
    Ok(match kind {
        "system" => RequestContext::system("sweeper", correlation)?,
        name => RequestContext::agent(name, permissions, correlation)?,
    })
}

// covers: H030, H031, H032, H036, H039, H042
#[tokio::test]
async fn the_application_checks_permissions_and_claim_ownership() -> TestResult {
    let harness = Harness::new("permissions")?;
    let (status, _) = harness
        .send(
            "POST",
            "/api/v1/ws/main/tasks",
            Some(&json!({"title": "Held", "status": "ready"})),
        )
        .await?;
    assert_eq!(status, 201);
    let (status, _) = harness
        .send("POST", "/api/v1/ws/main/tasks/WG-1/claim", Some(&json!({})))
        .await?;
    assert_eq!(status, 200);

    // An agent with the default grants may not release or complete another actor's claim.
    let bot = actor("bot", None)?;
    let released = harness
        .server
        .blocking({
            let bot = bot.clone();
            move |app| app.task_release(&bot, Some("main"), "WG-1", &Value::Null)
        })
        .await;
    let denied = released.err().ok_or("the release was allowed")?;
    assert_eq!(denied.code, wg_types::ErrorCode::PermissionDenied);
    assert_eq!(denied.message, "task WG-1 is claimed by human:tester");
    let completed = harness
        .server
        .blocking(move |app| app.task_complete(&bot, Some("main"), "WG-1", &Value::Null))
        .await;
    let denied = completed.err().ok_or("the completion was allowed")?;
    assert_eq!(denied.code, wg_types::ErrorCode::PermissionDenied);

    // A read-only agent lacks the permission each write names, before the task is looked up.
    let reader = actor("reader", Some(wg_types::read_permissions()))?;
    let writes = [
        ("claim", "task:claim"),
        ("comment", "task:write"),
        ("complete", "task:complete"),
        ("dependency", "task:write"),
        ("repository", "task:write"),
    ];
    for (what, missing) in writes {
        let reader = reader.clone();
        let outcome = harness
            .server
            .blocking(move |app| match what {
                "claim" => app.task_claim(&reader, Some("main"), "WG-404", &Value::Null),
                "comment" => {
                    app.task_comment(&reader, Some("main"), "WG-404", &json!({"body": "x"}))
                }
                "complete" => app.task_complete(&reader, Some("main"), "WG-404", &Value::Null),
                "dependency" => app.task_add_dependency(
                    &reader,
                    Some("main"),
                    "WG-404",
                    &json!({"depends_on": "WG-9"}),
                ),
                _ => app.task_link_repository(
                    &reader,
                    Some("main"),
                    "WG-404",
                    &json!({"repository": "web"}),
                ),
            })
            .await;
        let denied = outcome.err().ok_or("a write was allowed")?;
        assert_eq!(denied.code, wg_types::ErrorCode::PermissionDenied, "{what}");
        assert!(
            denied.message.contains(missing),
            "{what}: {}",
            denied.message
        );
    }

    // A system process may complete any claim.
    let sweeper = actor("system", None)?;
    let done = harness
        .server
        .blocking(move |app| app.task_complete(&sweeper, Some("main"), "WG-1", &Value::Null))
        .await?;
    let text = wg_json::http_json(&done).map_err(|e| format!("{e:?}"))?;
    let done: Value = serde_json::from_str(&text)?;
    assert_eq!(done["status"], json!("awaiting_review"));
    Ok(())
}
