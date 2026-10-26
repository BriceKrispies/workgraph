//! UI01: `workgraph ui open` against a fake health endpoint on an ephemeral port. Every test passes
//! `--print` (or fails before a browser could open) and never uses the default port 8765.

#![allow(clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A server that answers every request with `status`, on a port of its own.
fn fake_server(status: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("the address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line.trim_end() != "" {
                line.clear();
            }
            let reply =
                format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    port
}

/// A port that nothing listens on: bound once, then released.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    listener.local_addr().expect("the address").port()
}

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-ui01-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config")).expect("the home");
    dir
}

/// Writes the configured server address into the home's `config.toml`.
fn configure_server(home: &Path, port: u16) {
    let text = format!("[server]\nhost = \"127.0.0.1\"\nport = {port}\n");
    std::fs::write(home.join("config").join("config.toml"), text).expect("the config");
}

fn ui(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env("COLUMNS", "120")
        .output()
        .expect("the binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

// covers: C192
#[test]
fn a_running_server_named_by_url_prints_the_page_url() {
    let home = temp_home("print");
    let port = fake_server("200 OK");
    let url = format!("http://127.0.0.1:{port}");
    let out = ui(
        &home,
        &["ui", "open", "decisions", "--url", &url, "--print"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), format!("{url}/decisions\n"));
}

// covers: C192
#[test]
fn json_output_reports_the_url_and_that_nothing_was_started() {
    let home = temp_home("json");
    let port = fake_server("200 OK");
    let url = format!("http://127.0.0.1:{port}/");
    let out = ui(
        &home,
        &[
            "--json",
            "ui",
            "open",
            "tasks/WG-12",
            "--url",
            &url,
            "--print",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let value: serde_json::Value = serde_json::from_str(text(&out.stdout).trim()).expect("JSON");
    assert_eq!(value["url"], format!("http://127.0.0.1:{port}/tasks/WG-12"));
    assert_eq!(value["started"], false);
}

// covers: C192
#[test]
fn a_configured_server_is_used_when_no_url_is_given() {
    let home = temp_home("config");
    let port = fake_server("200 OK");
    configure_server(&home, port);
    let out = ui(&home, &["ui", "open", "--print"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), format!("http://127.0.0.1:{port}/\n"));
}

// covers: C192
#[test]
fn a_url_that_nothing_answers_at_is_refused_and_never_started() {
    let home = temp_home("refuse");
    let url = format!("http://127.0.0.1:{}", closed_port());
    let out = ui(&home, &["ui", "open", "--url", &url, "--print"]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(err.contains("no Workgraph server at"), "{err}");
    assert!(err.contains("start one with `workgraph serve`"), "{err}");
}

// covers: C192
#[test]
fn no_start_refuses_when_the_configured_server_is_down() {
    let home = temp_home("nostart");
    configure_server(&home, closed_port());
    let out = ui(&home, &["ui", "open", "--no-start", "--print"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("no Workgraph server at http://127.0.0.1:"));
}

// covers: C192
#[test]
fn a_server_answering_with_an_error_status_does_not_count() {
    let home = temp_home("status");
    let port = fake_server("500 Internal Server Error");
    let url = format!("http://127.0.0.1:{port}");
    let out = ui(&home, &["ui", "open", "--url", &url, "--print"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("no Workgraph server at"));
}

// covers: C192
#[test]
fn a_timeout_that_is_not_a_number_is_a_usage_error() {
    let home = temp_home("timeout");
    let out = ui(&home, &["ui", "open", "--timeout", "soon", "--print"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--timeout"));
}

// covers: C192
#[test]
fn the_screenshot_subcommand_is_still_a_placeholder() {
    let home = temp_home("shot");
    let out = ui(&home, &["ui", "screenshot"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("not implemented"));
}
