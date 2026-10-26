//! IF-40 parity (WG-3746): the `X-Correlation-Id` header as the Python app reads it. `get_ctx`
//! keeps any text of up to 64 characters as the correlation id (Starlette reads the header bytes
//! as Latin-1 text, so the length is the byte count), and any other value gets a fresh id.
//!
//! The expectations were recorded from the Python app: `create_app` over a temporary
//! `WORKGRAPH_HOME`, driven by starlette's TestClient, reading the events the request wrote.

use std::error::Error;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::Request;
use serde_json::{Value, json};
use tower::ServiceExt;
use wg_runtime::App;
use wg_server::{Answer, ApiRoute, Server, ServerOptions, Ui, handler, router};

type TestResult = Result<(), Box<dyn Error>>;

const PORT: u16 = 8765;

/// A stand-in for H123 (`GET /ws/{ws}/events`): it answers with the request's correlation id.
fn correlation_stub() -> ApiRoute {
    ApiRoute {
        id: "H123",
        method: "GET",
        path: "/api/v1/ws/{ws}/events",
        status: 200,
        query: &[],
        body: None,
        handler: handler(|_server, request| async move {
            Ok(Answer::Json(wg_wire::convention::json_value(&json!({
                "correlation": request.context.correlation.as_str(),
            }))))
        }),
    }
}

fn server(name: &str) -> Result<Arc<Server>, Box<dyn Error>> {
    let home = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if40-correlation")
        .join(name);
    if home.exists() {
        std::fs::remove_dir_all(&home)?;
    }
    let app = App::open(Some(&home))?;
    let options = ServerOptions {
        host: "127.0.0.1".to_owned(),
        port: PORT,
        lan: None,
        ui: Ui::Dir(home.clone()),
        mcp: None,
        routes: vec![correlation_stub()],
    };
    Ok(Arc::new(Server::new(options, app)?))
}

/// The correlation id the request's context carries, as the events would record it.
async fn correlation_of(server: &Arc<Server>, header: &[u8]) -> Result<String, Box<dyn Error>> {
    let mut request = Request::builder()
        .uri("/api/v1/ws/default/events")
        .header("host", format!("127.0.0.1:{PORT}"))
        .header("authorization", format!("Bearer {}", server.policy.token))
        .header("x-correlation-id", header)
        .body(Body::empty())?;
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
    let response = router(Arc::clone(server)).oneshot(request).await?;
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?;
    Ok(body["correlation"].as_str().unwrap_or_default().to_owned())
}

fn is_uuid(text: &str) -> bool {
    wg_types::parse_id(text).is_ok()
}

// covers: O13
#[tokio::test]
async fn correlation_header_text_is_kept_up_to_64_bytes() -> TestResult {
    let server = server("text")?;
    // Python: any text, `len(correlation) <= 64`.
    assert_eq!(correlation_of(&server, b"not a uuid").await?, "not a uuid");
    assert_eq!(correlation_of(&server, b"abc").await?, "abc");
    let at_limit = "x".repeat(64);
    assert_eq!(
        correlation_of(&server, at_limit.as_bytes()).await?,
        at_limit
    );
    // Over the limit, or empty: a fresh id.
    let over = "x".repeat(65);
    assert!(is_uuid(&correlation_of(&server, over.as_bytes()).await?));
    assert!(is_uuid(&correlation_of(&server, b"").await?));
    Ok(())
}

// covers: O13
#[tokio::test]
async fn correlation_header_is_read_as_latin_1() -> TestResult {
    let server = server("latin1")?;
    // UTF-8 bytes of "café" (5 bytes): Starlette reads each byte as one character.
    assert_eq!(
        correlation_of(&server, "caf\u{e9}".as_bytes()).await?,
        "caf\u{c3}\u{a9}"
    );
    // A 4-byte emoji is 4 characters, so it is kept.
    let emoji = "\u{1f680}";
    assert_eq!(
        correlation_of(&server, emoji.as_bytes()).await?,
        "\u{f0}\u{9f}\u{9a}\u{80}"
    );
    // 33 two-byte characters are 66 bytes: over the limit.
    let long = "\u{e9}".repeat(33);
    assert!(is_uuid(&correlation_of(&server, long.as_bytes()).await?));
    Ok(())
}
