//! The decisions group (H125 to H130), replayed in-process against the router with the group
//! registered, on a real database with one workspace and one task. The documents are the ones
//! `api/routes/decisions.py` returns (checked against the Python application).

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::Request;
use serde_json::{Value, json};
use tower::ServiceExt;
use wg_model::Write;
use wg_model::entity::Workspace;
use wg_runtime::{App, human_context};
use wg_server::{Server, ServerOptions, Ui, groups, router};
use wg_types::Timestamp;

type TestResult = Result<(), Box<dyn Error>>;

const PORT: u16 = 8765;

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("decisions")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

struct Harness {
    server: Arc<Server>,
    token: String,
}

impl Harness {
    /// A server over a home with the workspace `acme` (prefix `AC`) and the task AC-1.
    fn new(name: &str) -> Result<Harness, Box<dyn Error>> {
        let mut app = App::open(Some(&scratch(name)?.join("home")))?;
        let at = Timestamp::parse_iso("2026-01-01T00:00:00Z")?;
        app.runtime
            .store
            .commit_decision(&wg_model::Decision::new(()).write(Write::insert(Workspace {
                id: wg_model::derive_id(&[7; 32], 0),
                slug: "acme".to_owned(),
                name: "Acme".to_owned(),
                description: String::new(),
                key_prefix: "AC".to_owned(),
                next_task_number: 1,
                settings: serde_json::Map::new(),
                archived_at: None,
                created_at: at,
                updated_at: at,
            })))?;
        app.task_create(
            &human_context(None)?,
            Some("acme"),
            &json!({"title": "Pick a database"}),
        )?;
        let ui = scratch(&format!("{name}-ui"))?;
        let server = Server::new(
            ServerOptions {
                host: "127.0.0.1".to_owned(),
                port: PORT,
                lan: None,
                ui: Ui::Dir(ui),
                mcp: None,
                routes: groups::routes_with(&[&wg_decisions::FEATURE]),
            },
            app,
        )?;
        let token = server.policy.token.clone();
        Ok(Harness {
            server: Arc::new(server),
            token,
        })
    }

    /// The status and JSON body of an authorized request.
    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), Box<dyn Error>> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("authorization", format!("Bearer {}", self.token));
        let body = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let mut request = builder.body(body)?;
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
        let response = router(Arc::clone(&self.server)).oneshot(request).await?;
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, usize::MAX).await?;
        Ok((parts.status.as_u16(), serde_json::from_slice(&bytes)?))
    }
}

const BASE: &str = "/api/v1/ws/acme";

// covers: H130, H129, H125, H126
#[tokio::test]
async fn request_list_and_get_a_decision() -> TestResult {
    let harness = Harness::new("request")?;
    let (status, asked) = harness
        .send(
            "POST",
            &format!("{BASE}/tasks/AC-1/decisions"),
            Some(json!({"question": " Which db? ", "options": [
                {"label": "SQLite", "detail": "embedded", "recommended": true},
                {"label": "Postgres", "id": "pg"},
            ]})),
        )
        .await?;
    assert_eq!(status, 201, "{asked}");
    assert_eq!(asked["question"], "Which db?");
    assert_eq!(asked["task_key"], "AC-1");
    assert_eq!(asked["options"][0]["id"], "a");
    assert_eq!(asked["options"][1]["id"], "pg");
    assert_eq!(asked["version"], 1);
    let id = asked["id"].as_str().ok_or("no id")?.to_owned();

    let (status, listed) = harness
        .send("GET", &format!("{BASE}/decisions"), None)
        .await?;
    assert_eq!(status, 200);
    assert_eq!(listed["items"][0]["id"], id);
    assert_eq!(
        listed["items"][0]["task"]["tags"],
        json!(["needs-decision"])
    );
    assert_eq!(listed["items"][0]["task"]["priority"], "medium");

    let (status, one) = harness
        .send("GET", &format!("{BASE}/decisions/{id}"), None)
        .await?;
    assert_eq!(status, 200);
    assert_eq!(one["id"], id);
    assert_eq!(one["task"]["key"], "AC-1");

    let (status, per_task) = harness
        .send("GET", &format!("{BASE}/tasks/AC-1/decisions"), None)
        .await?;
    assert_eq!(status, 200);
    assert_eq!(per_task["items"].as_array().map(Vec::len), Some(1));
    assert!(per_task["items"][0].get("task").is_none());

    let (status, none) = harness
        .send("GET", &format!("{BASE}/decisions?status=decided"), None)
        .await?;
    assert_eq!((status, none), (200, json!({"items": []})));
    Ok(())
}

// covers: H127, H128
#[tokio::test]
async fn choose_clears_the_tag_and_an_invalid_option_is_422() -> TestResult {
    let harness = Harness::new("choose")?;
    let (_, asked) = harness
        .send(
            "POST",
            &format!("{BASE}/tasks/AC-1/decisions"),
            Some(json!({"question": "Which?", "options": [{"label": "A"}, {"label": "B"}]})),
        )
        .await?;
    let id = asked["id"].as_str().ok_or("no id")?.to_owned();

    let (status, error) = harness
        .send(
            "POST",
            &format!("{BASE}/decisions/{id}/choose"),
            Some(json!({"option_id": "zzz"})),
        )
        .await?;
    assert_eq!(status, 422);
    assert_eq!(error["error"]["code"], "validation_failed");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("unknown option 'zzz'; options are: a, b"))
    );

    let (status, chosen) = harness
        .send(
            "POST",
            &format!("{BASE}/decisions/{id}/choose"),
            Some(json!({"option_id": "b", "note": "  ok "})),
        )
        .await?;
    assert_eq!(status, 200, "{chosen}");
    assert_eq!(chosen["status"], "decided");
    assert_eq!(chosen["chosen_option_id"], "b");
    assert_eq!(chosen["note"], "ok");
    assert_eq!(
        chosen["decided_by"]
            .as_str()
            .map(|a| a.starts_with("human:")),
        Some(true)
    );

    let (_, second) = harness
        .send(
            "POST",
            &format!("{BASE}/tasks/AC-1/decisions"),
            Some(json!({"question": "Again?"})),
        )
        .await?;
    let second_id = second["id"].as_str().ok_or("no id")?.to_owned();
    let (status, withdrawn) = harness
        .send(
            "POST",
            &format!("{BASE}/decisions/{second_id}/withdraw"),
            Some(json!({"reason": "moot"})),
        )
        .await?;
    assert_eq!(
        (status, withdrawn["status"].clone()),
        (200, json!("withdrawn"))
    );
    let (status, again) = harness
        .send(
            "POST",
            &format!("{BASE}/decisions/{second_id}/withdraw"),
            None,
        )
        .await?;
    assert_eq!(status, 422, "{again}");

    let (status, missing) = harness
        .send(
            "GET",
            &format!("{BASE}/decisions/00000000-0000-0000-0000-000000000000"),
            None,
        )
        .await?;
    assert_eq!(
        (status, missing["error"]["code"].clone()),
        (404, json!("not_found"))
    );
    Ok(())
}

// covers: H130
#[tokio::test]
async fn a_request_body_must_match_the_model() -> TestResult {
    let harness = Harness::new("bodies")?;
    let (status, error) = harness
        .send(
            "POST",
            &format!("{BASE}/tasks/AC-1/decisions"),
            Some(json!({"question": ""})),
        )
        .await?;
    assert_eq!(status, 422, "{error}");
    let (status, _) = harness
        .send(
            "POST",
            &format!("{BASE}/tasks/AC-9/decisions"),
            Some(json!({"question": "Q"})),
        )
        .await?;
    assert_eq!(status, 404);
    Ok(())
}
