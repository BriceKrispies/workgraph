//! The events group (IF-49): the history route (H123) and the server-sent stream (H124), replayed
//! in-process against the router with the group registered.
//!
//! `contracts/http/sse.json` (O08) is the recorded stream: the domain frames of the workspace log,
//! then a queue snapshot. The frame layout and the `data` keys of the recorded domain event are
//! checked against it.

use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request};
use serde_json::{Value, json};
use tower::ServiceExt;
use wg_model::entity::Workspace;
use wg_model::event::WorkspaceCreated;
use wg_model::{Decision, EventDraft, EventPayload, Stamp, Write, derive_id};
use wg_runtime::{App, RequestContext};
use wg_server::{Server, ServerOptions, Ui, groups, router};
use wg_types::{Id, WgError};
use wg_wire::dto::event::FIELDS;

type TestResult = Result<(), Box<dyn Error>>;

/// The port the server believes it is bound to (the allowed hosts name it). Nothing binds it.
const PORT: u16 = 8765;

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if49")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// `workspace create <slug>` as a decision: the row and its `workspace.created` event.
fn create_workspace(slug: &str, stamp: &Stamp) -> Result<Decision<Id>, WgError> {
    let ([workspace_id, event_id], _) = stamp.ids().take::<2>();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(workspace_id),
        workspace_id,
        EventPayload::WorkspaceCreated(WorkspaceCreated {
            slug: slug.to_owned(),
            name: slug.to_uppercase(),
            key_prefix: "WG".to_owned(),
        }),
    );
    Ok(Decision::new(workspace_id)
        .write(Write::insert(workspace(workspace_id, slug, stamp)))
        .event(event))
}

fn workspace(id: Id, slug: &str, stamp: &Stamp) -> Workspace {
    Workspace {
        id,
        slug: slug.to_owned(),
        name: slug.to_uppercase(),
        description: String::new(),
        key_prefix: "WG".to_owned(),
        next_task_number: 1,
        settings: Default::default(),
        archived_at: None,
        created_at: stamp.at,
        updated_at: stamp.at,
    }
}

/// Appends one workspace per slug, in order, so the log is `seq` 1, 2, ... in that order.
fn seeded_app(name: &str, slugs: &[&str]) -> Result<App, Box<dyn Error>> {
    let home = scratch(name)?;
    let mut app = App::open(Some(&home.join("home")))?;
    let ctx = RequestContext::human(Some("tester"), None, derive_id(&[9; 32], 0))?;
    for slug in slugs {
        app.runtime
            .decide(&ctx, |_| Ok(()), |_, stamp| create_workspace(slug, stamp))?;
    }
    Ok(app)
}

/// One server with the group's routes registered, and the API token.
struct Harness {
    server: Arc<Server>,
    token: String,
    /// The ids of the seeded workspaces, in seeding order.
    workspaces: Vec<Id>,
}

/// A status, the headers and the body text of a response.
struct Reply {
    status: u16,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_str(&self.body)?)
    }
}

impl Harness {
    fn new(name: &str, slugs: &[&str]) -> Result<Harness, Box<dyn Error>> {
        let mut app = seeded_app(name, slugs)?;
        let workspaces = slugs
            .iter()
            .map(|slug| app.workspace_id(slug))
            .collect::<Result<Vec<Id>, WgError>>()?;
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
            workspaces,
        })
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        authorized: bool,
    ) -> Result<Reply, Box<dyn Error>> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("127.0.0.1:{PORT}"));
        if authorized {
            builder = builder.header("authorization", format!("Bearer {}", self.token));
        }
        let mut request = builder.body(Body::empty())?;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
        let response = router(Arc::clone(&self.server)).oneshot(request).await?;
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, usize::MAX).await?;
        Ok(Reply {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body: String::from_utf8(bytes.to_vec())?,
        })
    }
}

/// The frames of a recorded or served stream: each block between blank lines, as its lines.
fn frames(body: &str) -> Vec<Vec<String>> {
    body.split("\r\n\r\n")
        .filter(|block| !block.is_empty())
        .map(|block| block.split("\r\n").map(str::to_owned).collect())
        .collect()
}

/// The recorded O08 exchange.
fn recording() -> Result<Value, Box<dyn Error>> {
    let text =
        std::fs::read_to_string(repo_root().join("contracts").join("http").join("sse.json"))?;
    let document: Value = serde_json::from_str(&text)?;
    let exchange = document["exchanges"]
        .as_array()
        .and_then(|list| list.iter().find(|e| e["id"] == json!("O08")))
        .cloned()
        .ok_or("no O08 recording")?;
    Ok(exchange)
}

// covers: H123
#[tokio::test]
async fn history_lists_the_workspace_events_as_event_out() -> TestResult {
    let harness = Harness::new("history", &["default", "ops"])?;
    let reply = harness
        .send("GET", "/api/v1/ws/default/events", true)
        .await?;
    assert_eq!(reply.status, 200);
    let list = reply.json()?;
    let events = list.as_array().ok_or("not a list")?;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event["seq"], json!(1));
    assert_eq!(event["event_type"], json!("workspace.created"));
    assert_eq!(event["actor"], json!("human:tester"));
    let mut keys: Vec<&str> = event
        .as_object()
        .ok_or("not an object")?
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut expected = FIELDS.to_vec();
    expected.sort_unstable();
    assert_eq!(keys, expected);

    let other = harness.send("GET", "/api/v1/ws/ops/events", true).await?;
    assert_eq!(other.json()?[0]["seq"], json!(2));
    Ok(())
}

// covers: H123
#[tokio::test]
async fn history_applies_its_filters_and_matches_no_malformed_entity() -> TestResult {
    let harness = Harness::new("filters", &["default"])?;
    let id = harness.workspaces[0].to_string();
    let by_type = harness
        .send("GET", "/api/v1/ws/default/events?entity_type=task", true)
        .await?;
    assert_eq!(by_type.json()?, json!([]));
    let by_prefix = harness
        .send("GET", "/api/v1/ws/default/events?prefix=workspace.", true)
        .await?;
    assert_eq!(by_prefix.json()?.as_array().map(Vec::len), Some(1));
    let by_entity = harness
        .send(
            "GET",
            &format!("/api/v1/ws/default/events?entity_id={id}"),
            true,
        )
        .await?;
    assert_eq!(by_entity.json()?.as_array().map(Vec::len), Some(1));
    let malformed = harness
        .send("GET", "/api/v1/ws/default/events?entity_id=not-an-id", true)
        .await?;
    assert_eq!(malformed.json()?, json!([]));
    let unauthorized = harness
        .send("GET", "/api/v1/ws/default/events", false)
        .await?;
    assert_eq!(unauthorized.status, 401);
    Ok(())
}

// covers: H124, O08
#[tokio::test]
async fn stream_replays_the_recorded_framing() -> TestResult {
    let exchange = recording()?;
    let harness = Harness::new("stream", &["default", "ops"])?;
    let reply = harness
        .send(
            "GET",
            "/api/v1/ws/default/events/stream?after_seq=0&max_events=2",
            true,
        )
        .await?;
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        exchange["response"]["headers"]["content-type"].as_str()
    );
    assert_eq!(
        reply
            .headers
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        exchange["response"]["headers"]["cache-control"].as_str()
    );

    let served = frames(&reply.body);
    let recorded = exchange["response"]["events"]
        .as_array()
        .ok_or("no recorded events")?;
    assert_eq!(served.len(), 2);
    // The domain frame: `id`, `event`, then the data line of the recorded event.
    assert_eq!(
        served[0][0],
        format!("id: {}", recorded[0]["id"].as_str().unwrap_or_default())
    );
    assert_eq!(served[0][1], "event: domain");
    let data = served[0][2].strip_prefix("data: ").ok_or("no data line")?;
    let event: Value = serde_json::from_str(data)?;
    let mut keys: Vec<&str> = event
        .as_object()
        .ok_or("not an object")?
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    let mut recorded_keys: Vec<&str> = recorded[0]["data"]
        .as_object()
        .ok_or("recorded data is not an object")?
        .keys()
        .map(String::as_str)
        .collect();
    recorded_keys.sort_unstable();
    assert_eq!(keys, recorded_keys);
    assert_eq!(event["seq"], json!(1));
    assert_eq!(event["data"]["slug"], json!("default"));
    assert_eq!(event["data"]["key_prefix"], json!("WG"));

    // The queue snapshot follows the domain frames of the first tick, without an id.
    assert_eq!(served[1], ["event: queue", "data: {}"]);
    assert_eq!(recorded[2]["event"], json!("queue"));
    Ok(())
}

// covers: H124
#[tokio::test]
async fn stream_without_after_seq_starts_after_the_latest_event() -> TestResult {
    let harness = Harness::new("latest", &["default", "ops"])?;
    // Event 2 is the last one; the default workspace has nothing after it, so the first tick
    // carries only the queue snapshot (no jobs yet, so the counts are empty).
    let reply = harness
        .send("GET", "/api/v1/ws/default/events/stream?max_events=1", true)
        .await?;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "event: queue\r\ndata: {}\r\n\r\n");
    Ok(())
}

// covers: H124
#[tokio::test]
async fn stream_needs_the_read_permission_and_a_known_workspace() -> TestResult {
    let harness = Harness::new("stream-auth", &["default"])?;
    let unauthorized = harness
        .send(
            "GET",
            "/api/v1/ws/default/events/stream?max_events=1",
            false,
        )
        .await?;
    assert_eq!(unauthorized.status, 401);
    let missing = harness
        .send("GET", "/api/v1/ws/nowhere/events/stream?max_events=1", true)
        .await?;
    assert_eq!(missing.status, 404);
    Ok(())
}
