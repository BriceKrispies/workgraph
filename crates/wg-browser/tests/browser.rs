#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Browser discovery and a real headless capture. The capture test skips when no Chrome, Edge or
//! Chromium is installed, and serves its page on a loopback port of its own.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;

use wg_browser::{CaptureOptions, find_browser, screenshot};
use wg_types::ErrorCode;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-browser-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

#[test]
fn an_explicit_existing_file_is_the_browser() {
    let fake = scratch("explicit").join("my-browser.exe");
    std::fs::write(&fake, b"").expect("fake browser file");
    let found = find_browser(Some(fake.to_str().expect("utf-8 path"))).expect("found");
    assert_eq!(found, fake);
}

#[test]
fn an_unknown_explicit_browser_is_not_found() {
    let err = find_browser(Some("definitely-not-a-browser-xyz")).expect_err("missing");
    assert_eq!(err.code, ErrorCode::ProviderUnavailable);
    assert_eq!(
        err.message,
        "browser not found: definitely-not-a-browser-xyz"
    );
}

#[test]
fn bad_options_fail_before_any_browser_is_looked_for() {
    let options = CaptureOptions {
        theme: Some("sepia".to_owned()),
        ..CaptureOptions::default()
    };
    let out = scratch("options").join("p.png");
    let err = screenshot(
        "http://127.0.0.1:1/",
        &out,
        &options,
        Some("definitely-not-a-browser-xyz"),
    )
    .expect_err("sepia is not a theme");
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(!out.exists());
}

/// Serves one HTML page on a loopback port for as long as the test binary runs.
fn serve_page() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local address");
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let body = "<!doctype html><title>wg</title>\
                        <body style='margin:0;background:#246'><h1>Hello</h1></body>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{addr}/")
}

#[test]
fn a_capture_writes_a_png_of_the_requested_viewport() {
    if find_browser(None).is_err() {
        eprintln!("skipped: no Chrome, Edge or Chromium installed");
        return;
    }
    let url = serve_page();
    let out = scratch("capture").join("page.png");
    let options = CaptureOptions {
        width: 800,
        height: 600,
        settle: 0.2,
        ..CaptureOptions::default()
    };
    let capture = screenshot(&url, &out, &options, None).expect("screenshot");
    assert_eq!(capture.path, out);
    assert_eq!((capture.width, capture.height), (800, 600));
    let data = std::fs::read(&out).expect("png written");
    assert_eq!(&data[..8], b"\x89PNG\r\n\x1a\n");
    let width = u32::from_be_bytes(data[16..20].try_into().expect("4 bytes"));
    let height = u32::from_be_bytes(data[20..24].try_into().expect("4 bytes"));
    assert_eq!((width, height), (800, 600));
}
