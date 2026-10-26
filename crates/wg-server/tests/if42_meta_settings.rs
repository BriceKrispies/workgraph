//! The meta and settings group (IF-42, first part): H001 health, H002 meta, H117 and H118
//! settings, replayed in-process against the router with the group registered.
//!
//! `contracts/http/loopback.json` records the 405 for `POST /api/v1/health`, which is replayed
//! here. The success bodies of these routes have no recording, so they are checked against the
//! documents the Python routes build (`api/routes/core.py` and `api/routes/ops.py`).

use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request};
use serde_json::{Value, json};
use tower::ServiceExt;
use wg_runtime::App;
use wg_server::{Server, ServerOptions, Ui, groups, router};

type TestResult = Result<(), Box<dyn Error>>;

/// The port the server believes it is bound to (the allowed hosts name it). Nothing binds it.
const PORT: u16 = 8765;

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if42")
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

/// One server with its own home, the group's routes registered, and the API token.
struct Harness {
    server: Arc<Server>,
    token: String,
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
    fn new(name: &str) -> Result<Harness, Box<dyn Error>> {
        let home = scratch(name)?;
        let app = App::open(Some(&home.join("home")))?;
        let ui = scratch(&format!("{name}-ui"))?;
        let server = Server::new(
            ServerOptions {
                host: "127.0.0.1".to_owned(),
                port: PORT,
                lan: None,
                ui: Ui::Dir(ui),
                mcp: None,
                routes: groups::routes_with(&[&wg_registry_feature::FEATURE]),
            },
            app,
        )?;
        let token = server.policy.token.clone();
        Ok(Harness {
            server: Arc::new(server),
            token,
        })
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        authorized: bool,
        body: Option<Value>,
    ) -> Result<Reply, Box<dyn Error>> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("127.0.0.1:{PORT}"));
        if authorized {
            builder = builder.header("authorization", format!("Bearer {}", self.token));
        }
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
        Ok(Reply {
            status: parts.status.as_u16(),
            headers: parts.headers,
            body: String::from_utf8(bytes.to_vec())?,
        })
    }
}

/// The error envelope's `code` and `message`.
fn error_of(reply: &Reply) -> Result<(String, String), Box<dyn Error>> {
    let value = reply.json()?;
    let error = &value["error"];
    Ok((
        error["code"].as_str().unwrap_or_default().to_owned(),
        error["message"].as_str().unwrap_or_default().to_owned(),
    ))
}

// covers: H001
#[tokio::test]
async fn health_answers_without_a_token() -> TestResult {
    let harness = Harness::new("health")?;
    let reply = harness.send("GET", "/api/v1/health", false, None).await?;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.json()?, json!({"status": "ok", "version": "0.1.0"}));
    Ok(())
}

// covers: H001, X7
#[tokio::test]
async fn health_post_is_the_recorded_405() -> TestResult {
    let recording: Value = serde_json::from_str(&std::fs::read_to_string(
        repo_root()
            .join("contracts")
            .join("http")
            .join("loopback.json"),
    )?)?;
    let exchange = recording["exchanges"]
        .as_array()
        .and_then(|list| list.iter().find(|e| e["id"] == json!("405")))
        .cloned()
        .ok_or("no 405 recording")?;
    let harness = Harness::new("health-405")?;
    let reply = harness.send("POST", "/api/v1/health", false, None).await?;
    assert_eq!(
        reply.status,
        exchange["response"]["status"].as_u64().unwrap_or(0) as u16
    );
    assert_eq!(reply.json()?, exchange["response"]["body"]);
    assert_eq!(
        reply.headers.get("allow").and_then(|v| v.to_str().ok()),
        exchange["response"]["headers"]["allow"].as_str()
    );
    Ok(())
}

// covers: H002
#[tokio::test]
async fn meta_reports_the_effective_configuration() -> TestResult {
    let harness = Harness::new("meta")?;
    let reply = harness.send("GET", "/api/v1/meta", true, None).await?;
    assert_eq!(reply.status, 200);
    let meta = reply.json()?;
    assert_eq!(meta["version"], json!("0.1.0"));
    assert_eq!(meta["default_workspace"], Value::Null);
    assert_eq!(meta["execution_enabled"], json!(false));
    assert_eq!(meta["auto_schedule"], json!(false));
    assert!(meta["agents"].as_array().is_some_and(|a| !a.is_empty()));
    assert!(meta["llm"]["classification"].is_object());
    assert!(meta["llm"]["embeddings"].is_object());

    let unauthorized = harness.send("GET", "/api/v1/meta", false, None).await?;
    assert_eq!(unauthorized.status, 401);
    Ok(())
}

// covers: H117
#[tokio::test]
async fn settings_document_has_config_and_paths() -> TestResult {
    let harness = Harness::new("settings-get")?;
    let reply = harness.send("GET", "/api/v1/settings", true, None).await?;
    assert_eq!(reply.status, 200);
    let document = reply.json()?;
    assert_eq!(
        document["config"]["execution"]["max_concurrent_runs"],
        json!(2)
    );
    assert!(
        document["paths"]["config_file"]
            .as_str()
            .is_some_and(|p| !p.is_empty())
    );
    assert!(
        document["paths"]["data_dir"]
            .as_str()
            .is_some_and(|p| !p.is_empty())
    );

    // A workspace without an overrides file reads the global configuration.
    let scoped = harness
        .send("GET", "/api/v1/settings?workspace=nope", true, None)
        .await?;
    assert_eq!(scoped.status, 200);

    let unauthorized = harness.send("GET", "/api/v1/settings", false, None).await?;
    assert_eq!(unauthorized.status, 401);
    Ok(())
}

// covers: H118, H117
#[tokio::test]
async fn settings_patch_is_saved_and_read_back() -> TestResult {
    let harness = Harness::new("settings-patch")?;
    let patch = json!({"values": {"execution.max_concurrent_runs": 3}});
    let reply = harness
        .send("PATCH", "/api/v1/settings", true, Some(patch))
        .await?;
    assert_eq!(reply.status, 200);
    let document = reply.json()?;
    assert_eq!(
        document["config"]["execution"]["max_concurrent_runs"],
        json!(3)
    );
    // The response is the whole configuration, as Python's model dump.
    assert!(document["config"]["agents"].is_object());

    let config_file = harness
        .send("GET", "/api/v1/settings", true, None)
        .await?
        .json()?["paths"]["config_file"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let saved = std::fs::read_to_string(&config_file)?;
    assert!(saved.contains("max_concurrent_runs = 3"), "{saved}");

    let read_back = harness.send("GET", "/api/v1/settings", true, None).await?;
    assert_eq!(
        read_back.json()?["config"]["execution"]["max_concurrent_runs"],
        json!(3)
    );
    Ok(())
}

// covers: H118
/// The error text of an invalid value, as the Python route answers it (a live TestClient capture,
/// `workgraph.api.routes.ops.patch_settings`: `invalid settings: ` and Pydantic's `str(exc)`).
#[tokio::test]
async fn settings_patch_error_text_is_pythons() -> TestResult {
    let harness = Harness::new("settings-text")?;
    let reply = harness
        .send(
            "PATCH",
            "/api/v1/settings",
            true,
            Some(json!({"values": {"execution.max_concurrent_runs": 0}})),
        )
        .await?;
    assert_eq!(reply.status, 422);
    let expected = "invalid settings: 1 validation error for GlobalConfig\n\
        execution.max_concurrent_runs\n  \
        Input should be greater than or equal to 1 [type=greater_than_equal, input_value=0, input_type=int]\n    \
        For further information visit https://errors.pydantic.dev/2.14/v/greater_than_equal";
    assert_eq!(error_of(&reply)?.1, expected);
    assert_eq!(reply.json()?["error"]["details"], json!({}));
    Ok(())
}

// covers: H118
#[tokio::test]
async fn settings_patch_refuses_what_it_must() -> TestResult {
    let harness = Harness::new("settings-refuse")?;

    let outside = harness
        .send(
            "PATCH",
            "/api/v1/settings",
            true,
            Some(json!({"values": {"server.port": 1}})),
        )
        .await?;
    assert_eq!(outside.status, 422);
    assert_eq!(
        error_of(&outside)?,
        (
            "validation_failed".to_owned(),
            "server.port cannot be edited from the API; use the config file".to_owned()
        )
    );

    let invalid = harness
        .send(
            "PATCH",
            "/api/v1/settings",
            true,
            Some(json!({"values": {"execution.max_concurrent_runs": 0}})),
        )
        .await?;
    assert_eq!(invalid.status, 422);
    assert_eq!(error_of(&invalid)?.0, "validation_failed");

    // The body is required: without `values` the request never reaches the handler.
    let missing = harness
        .send("PATCH", "/api/v1/settings", true, Some(json!({})))
        .await?;
    assert_eq!(missing.status, 422);
    assert_eq!(error_of(&missing)?.0, "validation_failed");

    let unauthorized = harness
        .send(
            "PATCH",
            "/api/v1/settings",
            false,
            Some(json!({"values": {}})),
        )
        .await?;
    assert_eq!(unauthorized.status, 401);
    Ok(())
}
