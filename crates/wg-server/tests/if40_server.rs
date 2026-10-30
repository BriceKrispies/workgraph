//! wg-server skeleton checks (IF-40): the CF-04 recordings of `contracts/http/` replayed
//! in-process against the router (security screen, 401/403/404/405/421/422 bodies, security
//! headers, the OpenAPI and Swagger pages, the SPA with and without a built UI, LAN pairing and
//! the device cookie), the `/mcp/` mount, the event-stream framing, and one request over a real
//! socket on a free port.
//!
//! The replay compares each response with its recording as the conformance replay does: the
//! status, the recorded headers, and the body (JSON compared as values, text as text), with the
//! recorder's `<token>` and `<device-cookie>` placeholders filled in.

use std::error::Error;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, Response};
use axum::routing::any;
use futures_util::stream;
use serde_json::{Value, json};
use tower::ServiceExt;
use wg_runtime::App;
use wg_server::spec::security::device_cookie_value;
use wg_server::{
    Answer, ApiRoute, LanOptions, Server, ServerOptions, SseEvent, Ui, event_stream, handler,
    router, serve,
};

type TestResult = Result<(), Box<dyn Error>>;

/// The headers the CF-04 recorder keeps.
const RECORDED_HEADERS: [&str; 11] = [
    "allow",
    "cache-control",
    "content-type",
    "location",
    "mcp-session-id",
    "referrer-policy",
    "set-cookie",
    "www-authenticate",
    "x-accel-buffering",
    "x-content-type-options",
    "x-frame-options",
];

/// The port the servers under test believe they are bound to (for the allowed hosts). Nothing
/// binds it: the in-process tests open no socket.
const PORT: u16 = 8765;

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if40")
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

fn recording(name: &str) -> Result<Vec<Value>, Box<dyn Error>> {
    let path = repo_root().join("contracts").join("http").join(name);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    Ok(doc["exchanges"].as_array().cloned().unwrap_or_default())
}

/// A stand-in for H123 (`GET /ws/{ws}/events`, IF-49): validation runs before the handler, so
/// the recorded 422 is reproduced without the events group.
fn events_stub() -> ApiRoute {
    ApiRoute {
        id: "H123",
        method: "GET",
        path: "/api/v1/ws/{ws}/events",
        status: 200,
        query: &[
            "after_seq",
            "entity_type",
            "entity_id",
            "prefix",
            "limit",
            "descending",
        ],
        body: None,
        handler: handler(|_server, request| async move {
            Ok(Answer::Json(wg_wire::convention::json_object(
                &request.params,
            )))
        }),
    }
}

struct Harness {
    server: Arc<Server>,
    token: String,
    /// The device cookie a paired client sends back.
    cookie: Option<String>,
    client: SocketAddr,
    host: String,
}

impl Harness {
    fn new(name: &str, options: ServerOptions) -> Result<Harness, Box<dyn Error>> {
        let home = scratch(name)?;
        let app = App::open(Some(&home.join("home")))?;
        let server = Arc::new(Server::new(options, app)?);
        let token = server.policy.token.clone();
        Ok(Harness {
            server,
            token,
            cookie: None,
            client: SocketAddr::from(([127, 0, 0, 1], 50000)),
            host: format!("127.0.0.1:{PORT}"),
        })
    }

    async fn send(&self, request: Request<Body>) -> Result<Response<Body>, Box<dyn Error>> {
        let mut request = request;
        request.extensions_mut().insert(ConnectInfo(self.client));
        Ok(router(Arc::clone(&self.server)).oneshot(request).await?)
    }

    /// Sends the recorded request and returns what differs from the recorded response.
    async fn replay(&mut self, exchange: &Value) -> Result<Vec<String>, Box<dyn Error>> {
        let req = &exchange["request"];
        let fill = |text: &str| text.replace("<token>", &self.token);
        let mut builder = Request::builder()
            .method(req["method"].as_str().unwrap_or("GET"))
            .uri(req["path"].as_str().unwrap_or("/"));
        let mut has_host = false;
        for (name, value) in req["headers"].as_object().cloned().unwrap_or_default() {
            has_host |= name == "host";
            builder = builder.header(name.as_str(), fill(value.as_str().unwrap_or("")));
        }
        if !has_host {
            builder = builder.header("host", self.host.as_str());
        }
        if let Some(cookie) = &self.cookie {
            builder = builder.header("cookie", cookie.as_str());
        }
        let body = req["body"].as_str().unwrap_or("").to_owned();
        let response = self.send(builder.body(Body::from(body))?).await?;
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, usize::MAX).await?;
        let mut differences = Vec::new();
        let recorded = &exchange["response"];
        if recorded["status"] != json!(parts.status.as_u16()) {
            differences.push(format!(
                "status {} != recorded {}",
                parts.status, recorded["status"]
            ));
        }
        let device = device_cookie_value(&self.token);
        for name in RECORDED_HEADERS {
            let ours = parts.headers.get(name).and_then(|v| v.to_str().ok());
            let theirs = recorded["headers"][name]
                .as_str()
                .map(|v| fill(v).replace("<device-cookie>", &device));
            if ours.map(str::to_owned) != theirs {
                differences.push(format!("header {name}: {ours:?} != recorded {theirs:?}"));
            }
        }
        if let Some(set) = parts
            .headers
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
        {
            self.cookie = set.split(';').next().map(str::to_owned);
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        match &recorded["body"] {
            Value::String(expected) => {
                if text != fill(expected) {
                    differences.push(format!("body {text:?} != recorded {expected:?}"));
                }
            }
            Value::Null => {
                if !text.is_empty() {
                    differences.push(format!("body {text:?} != recorded none"));
                }
            }
            expected => {
                let expected = if expected.get("$ref").is_some() {
                    wg_wire::contract::openapi()?
                } else {
                    expected.clone()
                };
                match serde_json::from_str::<Value>(&text) {
                    Ok(ours) if ours == expected => {}
                    _ => differences.push(format!("body {text} != recorded {expected}")),
                }
            }
        }
        Ok(differences)
    }
}

fn options(ui: Ui, routes: Vec<ApiRoute>) -> ServerOptions {
    ServerOptions {
        host: "127.0.0.1".to_owned(),
        port: PORT,
        lan: None,
        ui,
        mcp: None,
        routes,
    }
}

/// Replays `exchanges`, skipping the cases in `skip`; panics with every difference found.
async fn replay_all(harness: &mut Harness, exchanges: &[Value], skip: &[&str]) -> TestResult {
    let mut failures = Vec::new();
    let mut replayed = 0;
    for exchange in exchanges {
        let label = format!(
            "{} {} ({})",
            exchange["id"].as_str().unwrap_or("?"),
            exchange["case"].as_str().unwrap_or("?"),
            exchange["state"].as_str().unwrap_or("?")
        );
        if skip.iter().any(|case| label.contains(case)) {
            continue;
        }
        if let Some(client) = exchange["client"].as_str() {
            harness.client = client.parse()?;
        }
        let differences = harness.replay(exchange).await?;
        replayed += 1;
        if !differences.is_empty() {
            failures.push(format!("{label}:\n  {}", differences.join("\n  ")));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(replayed > 0);
    Ok(())
}

// covers: X1, X2, X3, X4, X5, X6, X7, O06, O07
#[tokio::test]
async fn loopback_recordings_replay() -> TestResult {
    let ui = scratch("loopback-ui-missing")?;
    let mut harness = Harness::new("loopback", options(Ui::Dir(ui), vec![events_stub()]))?;
    let exchanges = recording("loopback.json")?;
    // The JSON-RPC answer of the MCP transport is wg-mcp's (IF-30).
    replay_all(
        &mut harness,
        &exchanges,
        &["authorized, body is not JSON-RPC"],
    )
    .await
}

// covers: X4, X7
#[tokio::test]
async fn built_ui_recordings_replay() -> TestResult {
    let fixture = repo_root()
        .join("tools")
        .join("contracts")
        .join("fixtures")
        .join("web");
    let mut harness = Harness::new("loopback-ui", options(Ui::Dir(fixture), Vec::new()))?;
    replay_all(&mut harness, &recording("loopback-ui.json")?, &[]).await
}

fn lan_options(ui: &Path) -> ServerOptions {
    ServerOptions {
        lan: Some(LanOptions {
            address: "192.168.1.10".to_owned(),
            networks: Vec::new(),
            hostname: "workgraph-test".to_owned(),
            pairing_code: Some("ABCD-EFGH".to_owned()),
        }),
        ..options(Ui::Dir(ui.to_path_buf()), Vec::new())
    }
}

// covers: X4, X5, X6, X7
#[tokio::test]
async fn lan_recordings_replay() -> TestResult {
    let ui = scratch("lan-ui-missing")?;
    let exchanges = recording("lan.json")?;
    let by_state = |state: &str| -> Vec<Value> {
        exchanges
            .iter()
            .filter(|e| e["state"] == json!(state))
            .cloned()
            .collect()
    };
    let device: SocketAddr = "192.168.1.50:40000".parse()?;
    let host = format!("192.168.1.10:{PORT}");

    // One server: a device pairs, then reads; an outsider and a loopback client.
    let mut harness = Harness::new("lan", lan_options(&ui))?;
    harness.host = host.clone();
    harness.client = device;
    replay_all(&mut harness, &by_state("LAN device, not paired"), &[]).await?;
    assert!(harness.cookie.is_some(), "pairing set no cookie");
    replay_all(&mut harness, &by_state("LAN device, paired"), &[]).await?;
    harness.cookie = None;
    harness.client = "8.8.8.8:40000".parse()?;
    replay_all(
        &mut harness,
        &by_state("client outside the private networks"),
        &[],
    )
    .await?;
    harness.client = "127.0.0.1:40000".parse()?;
    replay_all(
        &mut harness,
        &by_state("loopback client on a LAN-enabled server"),
        &[],
    )
    .await?;

    // A second server: ten wrong codes lock pairing.
    let mut locked = Harness::new("lan-locked", lan_options(&ui))?;
    locked.host = host;
    locked.client = device;
    replay_all(
        &mut locked,
        &by_state("LAN device after ten wrong codes"),
        &[],
    )
    .await
}

fn get(path: &str, token: Option<&str>) -> Result<Request<Body>, Box<dyn Error>> {
    let builder = Request::builder()
        .uri(path)
        .header("host", format!("127.0.0.1:{PORT}"));
    let builder = match token {
        Some(token) => builder.header("authorization", format!("Bearer {token}")),
        None => builder,
    };
    Ok(builder.body(Body::empty())?)
}

async fn body_text(response: Response<Body>) -> Result<String, Box<dyn Error>> {
    Ok(String::from_utf8(
        to_bytes(response.into_body(), usize::MAX).await?.to_vec(),
    )?)
}

// covers: X6
#[tokio::test]
async fn mcp_mount_forwards_with_security() -> TestResult {
    let ui = scratch("mcp-ui-missing")?;
    let mcp = axum::Router::new().fallback(any(|| async { "from mcp" }));
    let harness = Harness::new(
        "mcp",
        ServerOptions {
            mcp: Some(mcp),
            ..options(Ui::Dir(ui.clone()), Vec::new())
        },
    )?;
    let token = harness.token.clone();
    let response = harness.send(get("/mcp/", Some(&token))?).await?;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["x-frame-options"], "DENY");
    assert!(response.headers().get("cache-control").is_none());
    assert_eq!(body_text(response).await?, "from mcp");
    let unauthorized = harness.send(get("/mcp/", None)?).await?;
    assert_eq!(unauthorized.status(), 401);
    // `/mcp` without the slash is the catch-all's: the JSON 404 for GET, 405 otherwise.
    let bare = harness.send(get("/mcp", Some(&token))?).await?;
    assert_eq!(bare.status(), 404);
    assert_eq!(
        body_text(bare).await?,
        r#"{"error":{"code":"not_found","message":"/mcp not found"}}"#
    );
    // On a LAN-enabled server, MCP stays loopback-only even with the token.
    let mut lan = Harness::new(
        "mcp-lan",
        ServerOptions {
            mcp: Some(axum::Router::new().fallback(any(|| async { "from mcp" }))),
            ..lan_options(&ui)
        },
    )?;
    lan.client = "192.168.1.50:40000".parse()?;
    lan.host = format!("192.168.1.10:{PORT}");
    let token = lan.token.clone();
    let mut request = get("/mcp/", Some(&token))?;
    request
        .headers_mut()
        .insert("host", format!("192.168.1.10:{PORT}").parse()?);
    assert_eq!(lan.send(request).await?.status(), 403);
    Ok(())
}

// covers: O07
#[tokio::test]
async fn starlette_routing_edges() -> TestResult {
    let ui = scratch("edges-ui-missing")?;
    let harness = Harness::new("edges", options(Ui::Dir(ui), vec![events_stub()]))?;
    let token = harness.token.clone();
    let send = |method: &str, path: &str| -> Result<Request<Body>, Box<dyn Error>> {
        Ok(Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())?)
    };
    // HEAD on a FastAPI route is a 405; on the OpenAPI page it is the page without a body.
    let head = harness.send(send("HEAD", "/")?).await?;
    assert_eq!(head.status(), 405);
    assert_eq!(head.headers()["allow"], "GET");
    let page = harness.send(send("HEAD", "/api/openapi.json")?).await?;
    assert_eq!(page.status(), 200);
    assert_eq!(body_text(page).await?, "");
    let post = harness.send(send("POST", "/api/openapi.json")?).await?;
    assert_eq!(post.status(), 405);
    assert_eq!(post.headers()["allow"], "HEAD, GET");
    // A GET to a path only another method serves reaches the SPA's JSON 404.
    let other = harness.send(send("GET", "/api/v1/ws/w/events/")?).await?;
    assert_eq!(other.status(), 404);
    // Percent-encoded paths are matched decoded.
    let encoded = harness.send(send("GET", "/%61pi/v1/nope")?).await?;
    assert_eq!(
        body_text(encoded).await?,
        r#"{"error":{"code":"not_found","message":"/api/v1/nope not found"}}"#
    );
    // Validated parameters reach the handler with defaults filled in; the last value wins.
    let ok = harness
        .send(send("GET", "/api/v1/ws/w/events?limit=abc&limit=7")?)
        .await?;
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.headers()["cache-control"], "no-store");
    let value: Value = serde_json::from_str(&body_text(ok).await?)?;
    assert_eq!(
        value,
        json!({"ws": "w", "after_seq": null, "entity_type": null, "entity_id": null,
               "prefix": null, "limit": 7, "descending": false})
    );
    Ok(())
}

// covers: O08
#[tokio::test]
async fn event_stream_frames() -> TestResult {
    let events = stream::iter(vec![
        SseEvent {
            event: Some("domain".to_owned()),
            id: Some("1".to_owned()),
            data: Some("{\"seq\":1}".to_owned()),
        },
        SseEvent {
            event: Some("queue".to_owned()),
            id: None,
            data: Some("{\"queued\":0}".to_owned()),
        },
    ]);
    let response = event_stream(events);
    assert_eq!(
        response.headers()["content-type"],
        "text/event-stream; charset=utf-8"
    );
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    assert_eq!(
        body_text(response).await?,
        "id: 1\r\nevent: domain\r\ndata: {\"seq\":1}\r\n\r\nevent: queue\r\ndata: {\"queued\":0}\r\n\r\n"
    );
    Ok(())
}

// covers: X1
#[tokio::test]
async fn serves_on_a_real_socket() -> TestResult {
    let ui = scratch("socket-ui-missing")?;
    let home = scratch("socket")?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    assert_ne!(port, 8765);
    let server = Arc::new(Server::new(
        ServerOptions {
            port,
            ..options(Ui::Dir(ui), Vec::new())
        },
        App::open(Some(&home))?,
    )?);
    let token = server.policy.token.clone();
    let task = tokio::spawn(serve(server, vec![listener]));
    let reply = tokio::task::spawn_blocking(move || -> Result<String, String> {
        use std::io::{Read, Write};
        let mut stream =
            std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        let request = format!(
            "GET /api/openapi.json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .map_err(|e| e.to_string())?;
        let mut text = String::new();
        stream
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        Ok(text)
    })
    .await??;
    task.abort();
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    assert!(reply.contains("\r\n\r\n{\"components\":"), "{reply}");
    Ok(())
}
