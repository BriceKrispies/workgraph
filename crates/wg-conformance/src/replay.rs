//! The CF-04 replay: sends the recorded requests of `contracts/http/*.json` to an
//! implementation and compares every response with the recording.
//!
//! Each file runs in a fresh sandbox, the way `tools/contracts/record_http.py` recorded it:
//! `init`, a fixture command where the recording needed one, then one `serve` for the
//! file's exchanges in order. Responses are scrubbed with the recorder's placeholders before
//! comparison. `loopback-ui.json` and `lan.json` are not replayed: the first needs the
//! recorder's web fixture, the second needs client addresses that a socket cannot fake.

use crate::http::{self, Exchange, RECORDED_HEADERS, Request, Scrubber, Server};
use crate::runner::{Implementation, Outcome, Sandbox, run_cli};
use serde_json::{Map, Value, json};
use std::path::Path;

/// The recordings, in the order the recorder writes them.
pub const FILES: [&str; 4] = ["loopback.json", "loopback-ui.json", "lan.json", "sse.json"];

/// One replayed exchange.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub id: String,
    pub case: String,
    pub result: Result<(), String>,
}

/// The replay of one recording file: its checks, or the reason it was skipped.
#[derive(Debug, Clone, PartialEq)]
pub struct FileReplay {
    pub file: String,
    pub skipped: Option<&'static str>,
    pub checks: Vec<Check>,
}

/// What a recording needs before its first exchange.
enum Fixture {
    /// Run these CLI arguments with `--json` after `init`.
    Run(&'static [&'static str]),
    Skip(&'static str),
}

fn fixture(file: &str) -> Fixture {
    match file {
        "loopback.json" => Fixture::Run(&[]),
        // The recorder created one task through the CLI before opening the stream.
        "sse.json" => Fixture::Run(&["task", "create", "HTTP recording event"]),
        "loopback-ui.json" => {
            Fixture::Skip("needs the recorder's web UI fixture (tools/contracts/fixtures/web)")
        }
        "lan.json" => Fixture::Skip("needs LAN client addresses, which a socket cannot fake"),
        _ => Fixture::Skip("no replay fixture for this file"),
    }
}

/// Replay one recording file (a name from [`FILES`]) found in `contracts`, against `imp`.
pub fn replay_file(
    imp: &Implementation,
    contracts: &Path,
    file: &str,
) -> Result<FileReplay, String> {
    let args = match fixture(file) {
        Fixture::Skip(why) => {
            return Ok(FileReplay {
                file: file.to_owned(),
                skipped: Some(why),
                checks: Vec::new(),
            });
        }
        Fixture::Run(args) => args,
    };
    let path = contracts.join(file);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let rows = doc
        .get("exchanges")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: no exchanges array", path.display()))?;
    let openapi_path = contracts.join("..").join("openapi.json");
    let openapi_text = std::fs::read_to_string(&openapi_path)
        .map_err(|e| format!("read {}: {e}", openapi_path.display()))?;
    let openapi: Value = serde_json::from_str(&openapi_text)
        .map_err(|e| format!("{}: {e}", openapi_path.display()))?;

    let sb = Sandbox::create()?;
    let checks = replay_in(imp, &sb, args, rows, &openapi);
    let _ = std::fs::remove_dir_all(&sb.root);
    Ok(FileReplay {
        file: file.to_owned(),
        skipped: None,
        checks: checks?,
    })
}

fn replay_in(
    imp: &Implementation,
    sb: &Sandbox,
    fixture: &[&str],
    rows: &[Value],
    openapi: &Value,
) -> Result<Vec<Check>, String> {
    setup(imp, sb, &["init"], false)?;
    if !fixture.is_empty() {
        setup(imp, sb, fixture, true)?;
    }
    let server = Server::start(imp, sb)?;
    let mut scrub = Scrubber::default();
    let home = sb.home.to_string_lossy().into_owned();
    scrub.add(&home, "<home>");
    scrub.add(&home.replace('\\', "/"), "<home>");
    scrub.add(&server.token, "<token>");
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    scrub.add(&user, "<user>");
    scrub.add(&format!(":{}", server.port), ":<port>");
    rows.iter()
        .map(|row| exchange_check(&server, &scrub, openapi, row))
        .collect()
}

/// Run a setup command that must succeed.
fn setup(imp: &Implementation, sb: &Sandbox, args: &[&str], json_mode: bool) -> Result<(), String> {
    let owned: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let Outcome { exit, stderr, .. } = run_cli(imp, sb, &owned, json_mode, None, &[])?;
    if exit == 0 {
        Ok(())
    } else {
        Err(format!(
            "setup `{}` exited {exit}: {}",
            owned.join(" "),
            stderr.trim()
        ))
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("recording has no string `{key}`"))
}

/// Send one recorded request and compare the response with the recorded one.
fn exchange_check(
    server: &Server,
    scrub: &Scrubber,
    openapi: &Value,
    row: &Value,
) -> Result<Check, String> {
    let id = str_field(row, "id")?.to_owned();
    let case = str_field(row, "case")?.to_owned();
    let request = row.get("request").ok_or("recording has no request")?;
    let response = row.get("response").ok_or("recording has no response")?;
    let method = str_field(request, "method")?;
    let path = str_field(request, "path")?;
    // The recorder wrote `<token>` where the real token was sent.
    let headers: Vec<(String, String)> = request
        .get("headers")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let v = v
                        .as_str()
                        .unwrap_or_default()
                        .replace("<token>", &server.token);
                    (k.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default();
    let body = request
        .get("body")
        .and_then(Value::as_str)
        .map(|s| s.replace("<token>", &server.token));
    let expected_events = response.get("events").and_then(Value::as_array);
    let mut headers = headers;
    // The SSE recording lists no headers, but the recorder's client sent the token.
    if expected_events.is_some()
        && !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    {
        headers.push((
            "authorization".to_owned(),
            format!("Bearer {}", server.token),
        ));
    }
    let req = Request {
        method,
        path,
        headers: &headers,
        body: body.as_deref().map(str::as_bytes),
    };
    let got = http::send(server.port, &req, expected_events.map(Vec::len));
    let result = match got {
        Ok(ex) => compare(&ex, response, expected_events, scrub, openapi, &id),
        Err(e) => Err(format!("request failed: {e}")),
    };
    Ok(Check { id, case, result })
}

fn compare(
    ex: &Exchange,
    recorded: &Value,
    expected_events: Option<&Vec<Value>>,
    scrub: &Scrubber,
    openapi: &Value,
    id: &str,
) -> Result<(), String> {
    let want_status = recorded.get("status").and_then(Value::as_u64).unwrap_or(0);
    if u64::from(ex.status) != want_status {
        return Err(format!("status {} wanted {want_status}", ex.status));
    }

    let mut headers = Map::new();
    for name in RECORDED_HEADERS {
        let values: Vec<&str> = ex
            .headers
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect();
        if values.is_empty() {
            continue;
        }
        let value = if name == "mcp-session-id" {
            "<session>".to_owned()
        } else {
            scrub.text(&values.join(", "))
        };
        headers.insert(name.to_owned(), Value::String(value));
    }
    let want_headers = recorded
        .get("headers")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if Value::Object(headers.clone()) != want_headers {
        return Err(format!(
            "headers {} wanted {want_headers}",
            Value::Object(headers)
        ));
    }

    let (got, want) = match expected_events {
        Some(events) => (
            Value::Array(ex.events.iter().map(|e| scrub.value(e)).collect()),
            Value::Array(events.clone()),
        ),
        None => {
            let is_json = ex
                .header("content-type")
                .is_some_and(|c| c.contains("application/json"));
            let mut body = if is_json {
                serde_json::from_slice::<Value>(&ex.body)
                    .map_err(|e| format!("body is not JSON: {e}"))?
            } else {
                Value::String(String::from_utf8_lossy(&ex.body).into_owned())
            };
            // X1 records the served OpenAPI document as a reference to the contract file.
            if id == "X1" && ex.status == 200 && &body == openapi {
                body = json!({"$ref": "contracts/openapi.json"});
            }
            (
                scrub.value(&body),
                recorded.get("body").cloned().unwrap_or(Value::Null),
            )
        }
    };
    if got != want {
        return Err(format!(
            "body differs: got {} wanted {}",
            short(&got),
            short(&want)
        ));
    }
    Ok(())
}

fn short(v: &Value) -> String {
    let text = v.to_string();
    if text.chars().count() > 240 {
        format!("{}...", text.chars().take(240).collect::<String>())
    } else {
        text
    }
}

/// The number of failed checks across files.
pub fn failed(files: &[FileReplay]) -> usize {
    files
        .iter()
        .flat_map(|f| &f.checks)
        .filter(|c| c.result.is_err())
        .count()
}

/// Human-readable report: one line per file, one per failed check.
pub fn report_text(files: &[FileReplay]) -> String {
    let mut out = String::new();
    for f in files {
        if let Some(why) = f.skipped {
            out.push_str(&format!("{}: skipped ({why})\n", f.file));
            continue;
        }
        let bad = f.checks.iter().filter(|c| c.result.is_err()).count();
        out.push_str(&format!(
            "{}: {} exchanges, {bad} failed\n",
            f.file,
            f.checks.len()
        ));
        for c in f.checks.iter().filter(|c| c.result.is_err()) {
            if let Err(why) = &c.result {
                out.push_str(&format!("  FAIL {} {}: {why}\n", c.id, c.case));
            }
        }
    }
    out
}
