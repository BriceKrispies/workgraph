//! The stacks group (H062 to H070), replayed in-process against the router with the group
//! registered, on a real database and real temporary git repositories. The documents are the ones
//! `api/routes/stacks.py` returns: the scenario is the one the differential script
//! (`stack_trace.py`) replays against the Python application, whose results the assertions pin.
//!
//! Set `STACK_TRACE_OUT` to a path to write the whole trace (every request and reply, the
//! readmit and the events it wrote) for comparison with the Python trace.

use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::Request;
use serde_json::{Value, json};
use tower::ServiceExt;
use wg_model::entity::{Repository, RepositoryCheckout, StackItem, Workspace};
use wg_model::{Decision, Write};
use wg_runtime::spec::events::event_out;
use wg_runtime::{App, human_context};
use wg_server::{Server, ServerOptions, Ui, groups, router};
use wg_types::{AccessMode, StackItemState, Timestamp};

type TestResult = Result<(), Box<dyn Error>>;

const PORT: u16 = 8765;
const BASE: &str = "/api/v1/ws/main";

/// The commit `make_repos.py` and this test both create in `alpha`: fixed author, dates and tree.
const ALPHA_HEAD: &str = "669f845216e215c7f7f1004b9214e029720845eb";

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if53")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn git(path: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:01+00:00")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:01+00:00")
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A repository with one commit on `main`.
fn make_repo(root: &Path, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let path = root.join(name);
    std::fs::create_dir_all(&path)?;
    git(&path, &["init", "-q", "-b", "main"])?;
    git(&path, &["config", "core.autocrlf", "false"])?;
    std::fs::write(path.join("README.md"), format!("# {name}\n"))?;
    git(&path, &["add", "-A"])?;
    git(&path, &["commit", "-q", "-m", "initial commit"])?;
    Ok(path)
}

fn id(seed: u8, slot: u32) -> wg_types::Id {
    wg_model::derive_id(&[seed; 32], slot)
}

struct Harness {
    server: Arc<Server>,
    token: String,
    repos: PathBuf,
    home: PathBuf,
    trace: Vec<Value>,
}

impl Harness {
    /// A server over a home with the workspace `main` (prefix `WG`) and the repositories `alpha`
    /// and `beta` registered at their checkouts.
    fn new(name: &str) -> Result<Harness, Box<dyn Error>> {
        let root = scratch(name)?;
        let home = root.join("home");
        let repos = root.join("repos");
        let mut app = App::open(Some(&home))?;
        let mut config = app.config()?;
        config.insert(
            "llm".to_owned(),
            json!({"auto_classify": false, "auto_embed": false}),
        );
        app.save_config(&config)?;
        let at = Timestamp::parse_iso("2026-01-01T00:00:00Z")?;
        let workspace = id(7, 0);
        let mut decision = Decision::new(()).write(Write::insert(Workspace {
            id: workspace,
            slug: "main".to_owned(),
            name: "Main".to_owned(),
            description: String::new(),
            key_prefix: "WG".to_owned(),
            next_task_number: 1,
            settings: serde_json::Map::new(),
            archived_at: None,
            created_at: at,
            updated_at: at,
        }));
        for (index, repo) in ["alpha", "beta"].into_iter().enumerate() {
            let repository = id(8, u32::try_from(index)?);
            let path = make_repo(&repos, repo)?;
            decision = decision
                .write(Write::insert(Repository {
                    id: repository,
                    workspace_id: workspace,
                    name: repo.to_owned(),
                    description: String::new(),
                    identity_fingerprint: None,
                    default_branch: Some("main".to_owned()),
                    tracked_branches: rpds::VectorSync::new_sync(),
                    access_mode: AccessMode::ReadWrite,
                    health_status: "unknown".to_owned(),
                    health_detail: serde_json::Map::new(),
                    health_checked_at: None,
                    is_shallow: false,
                    last_scanned_at: None,
                    archived_at: None,
                    metadata: serde_json::Map::new(),
                    created_at: at,
                    updated_at: at,
                }))
                .write(Write::insert(RepositoryCheckout {
                    id: id(9, u32::try_from(index)?),
                    repository_id: repository,
                    path: path.display().to_string(),
                    kind: "clone".to_owned(),
                    is_primary: true,
                    access_mode: None,
                    available: true,
                    last_seen_at: None,
                    created_at: at,
                    updated_at: at,
                }));
        }
        app.runtime.store.commit_decision(&decision)?;
        let ui = scratch(&format!("{name}-ui"))?;
        let server = Server::new(
            ServerOptions {
                host: "127.0.0.1".to_owned(),
                port: PORT,
                lan: None,
                ui: Ui::Dir(ui),
                mcp: None,
                routes: groups::routes_with(&[
                    &wg_tasks_feature::FEATURE,
                    &wg_stacks_feature::FEATURE,
                ]),
            },
            app,
        )?;
        let token = server.policy.token.clone();
        Ok(Harness {
            server: Arc::new(server),
            token,
            repos,
            home,
            trace: Vec::new(),
        })
    }

    /// Creates a task through the application, as the scenario's seeds do.
    async fn seed(&self, body: Value) -> TestResult {
        self.server
            .blocking(move |app| {
                wg_tasks_feature::service::task_create(
                    app,
                    &human_context(None)?,
                    Some("main"),
                    &body,
                )
            })
            .await?;
        Ok(())
    }

    /// The status and JSON body of an authorized request, recorded in the trace.
    async fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), Box<dyn Error>> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("authorization", format!("Bearer {}", self.token))
            .header("x-workgraph-actor", "tester")
            .header("x-correlation-id", "corr");
        let payload = match &body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let mut request = builder.body(payload)?;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
        let response = router(Arc::clone(&self.server)).oneshot(request).await?;
        let (parts, reply) = response.into_parts();
        let bytes = to_bytes(reply, usize::MAX).await?;
        let status = parts.status.as_u16();
        let json: Value = serde_json::from_slice(&bytes)?;
        self.trace.push(json!({
            "method": method, "path": path, "body": body, "status": status, "json": json,
        }));
        Ok((status, json))
    }
}

fn keys(value: &Value, field: &str) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item[field].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

// covers: H062, H063, H064, H065, H066, H067, H068, H069, H070
#[tokio::test]
async fn the_stack_routes_answer_as_the_python_application_does() -> TestResult {
    let mut h = Harness::new("routes")?;
    h.seed(json!({"title": "Parent work", "repositories": ["alpha"]}))
        .await?;
    for title in ["First change", "Second change"] {
        h.seed(
            json!({"title": title, "parent": "WG-1", "repositories": ["alpha"], "status": "ready"}),
        )
        .await?;
    }
    h.seed(json!({"title": "Childless parent", "repositories": ["alpha"]}))
        .await?;

    let (status, listed) = h.call("GET", &format!("{BASE}/stacks"), None).await?;
    assert_eq!((status, listed), (200, json!([])));
    let (status, none) = h
        .call(
            "POST",
            &format!("{BASE}/stacks"),
            Some(json!({"parent": "WG-4"})),
        )
        .await?;
    assert_eq!(status, 422, "{none}");
    assert_eq!(none["error"]["message"], "WG-4 has no child tasks to stack");

    let (status, created) = h
        .call(
            "POST",
            &format!("{BASE}/stacks"),
            Some(json!({"parent": "WG-1"})),
        )
        .await?;
    assert_eq!(status, 201, "{created}");
    assert_eq!(created["status"], "open");
    assert_eq!(created["parent_key"], "WG-1");
    assert_eq!(created["repositories"][0]["base_sha"], ALPHA_HEAD);
    assert_eq!(
        created["repositories"][0]["integration_branch"],
        "wg/stack/wg-1"
    );
    assert_eq!(keys(&created["items"], "task_key"), ["WG-2", "WG-3"]);
    assert_eq!(keys(&created["items"], "state"), ["pending", "pending"]);
    let (status, again) = h
        .call(
            "POST",
            &format!("{BASE}/stacks"),
            Some(json!({"parent": "WG-1"})),
        )
        .await?;
    assert_eq!(
        (status, again["error"]["message"].clone()),
        (409, json!("WG-1 already has an active stack"))
    );

    // A prepared integration worktree reports its tip.
    let stack_id = created["id"].as_str().ok_or("no stack id")?.to_owned();
    let worktree = h
        .home
        .join("data")
        .join("worktrees")
        .join("stacks")
        .join(&stack_id)
        .join("alpha");
    std::fs::create_dir_all(worktree.parent().ok_or("no parent")?)?;
    git(
        &h.repos.join("alpha"),
        &[
            "worktree",
            "add",
            "-q",
            &worktree.display().to_string(),
            "-b",
            "wg/stack/wg-1",
        ],
    )?;
    let (_, listed) = h.call("GET", &format!("{BASE}/stacks"), None).await?;
    assert_eq!(listed[0]["repositories"][0]["integration_tip"], ALPHA_HEAD);
    let (_, shown) = h.call("GET", &format!("{BASE}/stacks/WG-1"), None).await?;
    assert_eq!(shown["id"], created["id"]);
    assert_eq!(shown["repositories"][0]["integration_tip"], ALPHA_HEAD);
    let (_, holding) = h
        .call("GET", &format!("{BASE}/tasks/WG-2/stacks"), None)
        .await?;
    assert_eq!(holding[0]["integration_tip"], Value::Null);
    assert_eq!(
        holding[0]["repositories"][0]["integration_tip"],
        Value::Null
    );
    let (_, empty) = h
        .call("GET", &format!("{BASE}/tasks/WG-1/stacks"), None)
        .await?;
    assert_eq!(empty, json!([]));

    h.seed(json!({"title": "Late change", "parent": "WG-1", "repositories": ["alpha"], "status": "ready"}))
        .await?;
    h.seed(json!({"title": "Beta change", "parent": "WG-1", "repositories": ["beta"]}))
        .await?;
    h.seed(json!({"title": "Loose task", "repositories": ["alpha"]}))
        .await?;
    let items = format!("{BASE}/stacks/WG-1/items");
    let (status, added) = h
        .call("POST", &items, Some(json!({"task": "WG-5"})))
        .await?;
    assert_eq!(status, 201, "{added}");
    assert_eq!(keys(&added["items"], "task_key"), ["WG-2", "WG-3", "WG-5"]);
    let expectations = [
        (json!({"task": "WG-5"}), 409),
        (json!({"task": "WG-7"}), 422),
        (json!({"task": "WG-6"}), 422),
        (json!({"task": "WG-99"}), 404),
        (json!({"task": "WG-5", "extra": 1}), 422),
    ];
    for (body, expected) in expectations {
        let (status, reply) = h.call("POST", &items, Some(body)).await?;
        assert_eq!(status, expected, "{reply}");
    }
    let (status, removed) = h.call("DELETE", &format!("{items}/WG-5"), None).await?;
    assert_eq!(status, 200, "{removed}");
    assert_eq!(keys(&removed["items"], "task_key"), ["WG-2", "WG-3"]);
    for (task, expected) in [("WG-5", 404), ("WG-99", 404)] {
        let (status, reply) = h.call("DELETE", &format!("{items}/{task}"), None).await?;
        assert_eq!(status, expected, "{reply}");
    }
    for path in ["stacks/WG-99", "stacks/WG-4"] {
        let (status, reply) = h.call("GET", &format!("{BASE}/{path}"), None).await?;
        assert_eq!(status, 404, "{reply}");
    }
    let (status, history) = h
        .call("GET", &format!("{BASE}/stacks/WG-1/history"), None)
        .await?;
    assert_eq!(status, 200);
    assert_eq!(
        keys(&history, "event_type"),
        ["stack.created", "stack.item_added", "stack.item_removed"]
    );
    assert_eq!(history[0]["data"]["base_shas"]["alpha"], ALPHA_HEAD);

    let (status, first) = h
        .call("POST", &format!("{BASE}/stacks/WG-1/integrate"), None)
        .await?;
    assert_eq!(status, 202, "{first}");
    let (_, second) = h
        .call("POST", &format!("{BASE}/stacks/WG-1/integrate"), None)
        .await?;
    assert_eq!(first, second, "an unchanged stack collapses into one job");
    assert_eq!(first["stack"], "WG-1");
    assert_eq!(first["jobs"][0]["repository"], "alpha");
    let (status, verify) = h
        .call("POST", &format!("{BASE}/stacks/WG-1/verify"), None)
        .await?;
    assert_eq!(status, 202, "{verify}");
    let (_, verify_again) = h
        .call("POST", &format!("{BASE}/stacks/WG-1/verify"), None)
        .await?;
    assert_eq!(verify, verify_again);
    for action in ["integrate", "verify"] {
        let (status, reply) = h
            .call("POST", &format!("{BASE}/stacks/WG-4/{action}"), None)
            .await?;
        assert_eq!(
            (status, reply["error"]["message"].clone()),
            (404, json!("WG-4 has no active stack"))
        );
    }
    let (status, all) = h
        .call("GET", &format!("{BASE}/stacks?include_inactive=true"), None)
        .await?;
    assert_eq!((status, all.as_array().map(Vec::len)), (200, Some(1)));
    let (status, _) = h
        .call(
            "GET",
            &format!("{BASE}/stacks?include_inactive=maybe"),
            None,
        )
        .await?;
    assert_eq!(status, 422);
    let (status, _) = h.call("GET", "/api/v1/ws/nope/stacks", None).await?;
    assert_eq!(status, 404);
    h.call("GET", &format!("{BASE}/stacks/WG-1/history"), None)
        .await?;

    // Item transitions and the readmit, through the application (the CLI's path).
    let stack_uuid = wg_types::parse_id(shown["id"].as_str().ok_or("no id")?)?;
    let first_task = wg_types::parse_id(shown["items"][0]["task_id"].as_str().ok_or("no task")?)?;
    let ejected = h
        .server
        .blocking(move |app| {
            let marker = app.runtime.store.read(|session| session.latest_seq())?;
            app.runtime.store.write(|session| {
                let items = wg_stacks_feature::service::reads::stack_items(session, stack_uuid)?;
                let first = items
                    .iter()
                    .find(|item| item.task_id == first_task)
                    .cloned()
                    .ok_or_else(|| {
                        wg_types::WgError::new(wg_types::ErrorCode::WorkgraphError, "no item")
                    })?;
                session.apply_decision(&Decision::new(()).write(Write::update(StackItem {
                    state: StackItemState::Ejected,
                    merge_order: Some(1),
                    ..first
                })))?;
                Ok(())
            })?;
            Ok(marker)
        })
        .await?;
    let readmitted = h
        .server
        .blocking(move |app| {
            let ctx = human_context(Some("tester"))?;
            let out = wg_stacks_feature::service::readmit(app, &ctx, Some("main"), "WG-1", "WG-2")?;
            let second =
                wg_stacks_feature::service::readmit(app, &ctx, Some("main"), "WG-1", "WG-3");
            let again =
                wg_stacks_feature::service::readmit(app, &ctx, Some("main"), "WG-1", "WG-2");
            let events = app.runtime.store.read(|session| {
                session.query_events(&wg_store::EventQuery {
                    after_seq: Some(ejected),
                    limit: 100_000,
                    ..wg_store::EventQuery::default()
                })
            })?;
            let render = |value: &wg_json::Value| -> Result<Value, wg_types::WgError> {
                let text = wg_json::http_json(value).map_err(|err| {
                    wg_types::WgError::new(wg_types::ErrorCode::WorkgraphError, format!("{err:?}"))
                })?;
                serde_json::from_str(&text).map_err(|err| {
                    wg_types::WgError::new(wg_types::ErrorCode::WorkgraphError, err.to_string())
                })
            };
            let rendered: Vec<Value> = events
                .iter()
                .map(|event| render(&event_out(event)))
                .collect::<Result<_, _>>()?;
            Ok((
                render(&out)?,
                second.is_ok(),
                again.err().map(|err| (err.message, err.details)),
                rendered,
            ))
        })
        .await?;
    let (readmit_out, second_ok, again_error, events) = readmitted;
    assert!(second_ok, "a pending child can be readmitted");
    assert_eq!(
        again_error.map(|(message, _)| message),
        Some("WG-2 is ready; only an ejected or pending child can be readmitted".to_owned())
    );
    let first_state = readmit_out["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["task_key"] == "WG-2"))
        .map(|item| item["state"].clone());
    assert_eq!(first_state, Some(json!("ready")));
    let types: Vec<&str> = events
        .iter()
        .filter_map(|event| event["event_type"].as_str())
        .collect();
    assert_eq!(
        types,
        [
            "stack.item_state_changed",
            "task.status_changed",
            "task.status_changed",
            "stack.item_state_changed",
            "task.status_changed",
            "task.status_changed",
        ]
    );
    h.trace.push(json!({"method": "SERVICE", "path": "readmit WG-1 WG-2", "body": null, "status": 0, "json": readmit_out}));
    h.trace.push(json!({"method": "EVENTS", "path": "after readmit", "body": null, "status": 0, "json": events}));

    if let Ok(path) = std::env::var("STACK_TRACE_OUT") {
        std::fs::write(path, serde_json::to_string_pretty(&h.trace)?)?;
    }
    Ok(())
}
