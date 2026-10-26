//! IF-11: the `job` and `worker` commands, run through the real binary against a real SQLite
//! database in a temporary home.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if11-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn run(home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .arg("--home")
        .arg(home)
        .arg("--json")
        .args(args)
        .env("COLUMNS", "120")
        .output()
        .expect("the binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn json(home: &Path, args: &[&str], expect: i32) -> Value {
    let (code, stdout, stderr) = run(home, args);
    assert_eq!(code, expect, "{args:?}: {stdout}{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
}

fn enqueue(home: &Path, kind: &str, payload: &str) -> String {
    json(home, &["job", "enqueue", kind, "--payload", payload], 0)["job_id"]
        .as_str()
        .expect("a job id")
        .to_owned()
}

// covers: C102, C103, C105, C106
#[test]
fn enqueue_list_show_and_stats_describe_the_queued_job() {
    let home = temp_home("basics");
    let id = enqueue(&home, "noop", r#"{"sleep": 0}"#);
    let again = json(
        &home,
        &["job", "enqueue", "noop", "--key", "k1", "--payload", "{}"],
        0,
    );
    let keyed = again["job_id"].as_str().expect("a job id").to_owned();
    let repeat = json(
        &home,
        &["job", "enqueue", "noop", "--key", "k1", "--payload", "{}"],
        0,
    );
    assert_eq!(repeat["job_id"], keyed, "a key names one job");

    let listing = json(&home, &["job", "list"], 0);
    assert_eq!(listing["total"], 2);
    let first = &listing["items"][0];
    assert_eq!(first["status"], "queued");
    assert_eq!(first["max_attempts"], 5);
    assert!(
        first["created_at"]
            .as_str()
            .is_some_and(|text| text.ends_with("+00:00")),
        "datetimes print as isoformat: {first}"
    );

    let shown = json(&home, &["job", "show", &id], 0);
    assert_eq!(shown["kind"], "noop");
    assert_eq!(shown["payload"], serde_json::json!({"sleep": 0}));
    assert_eq!(shown["runs"], serde_json::json!([]));

    let stats = json(&home, &["job", "stats"], 0);
    assert_eq!(stats, serde_json::json!({"queued": 2}));
}

// covers: C179, C105, C177
#[test]
fn worker_run_once_completes_a_noop_and_records_its_run() {
    let home = temp_home("worker-once");
    let id = enqueue(&home, "noop", r#"{"sleep": 0}"#);
    let (code, stdout, stderr) = run_human(&home, &["worker", "run", "--once"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("worker stopped: 1 completed, 0 failed"),
        "{stdout}"
    );

    let shown = json(&home, &["job", "show", &id], 0);
    assert_eq!(shown["status"], "succeeded");
    assert_eq!(shown["runs"][0]["status"], "succeeded");
    assert_eq!(shown["runs"][0]["attempt"], 1);

    let workers = json(&home, &["worker", "list"], 0);
    let rows = workers.as_array().expect("a list of workers");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status"], "stopped");
    assert_eq!(rows[0]["completed"], 1);
}

/// The human form of one command, without `--json`.
fn run_human(home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env("COLUMNS", "120")
        .output()
        .expect("the binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

// covers: C100, C104
#[test]
fn cancel_and_retry_follow_the_queue_rules() {
    let home = temp_home("cancel-retry");
    let id = enqueue(&home, "noop", "{}");

    let cancelled = json(&home, &["job", "cancel", &id], 0);
    assert_eq!(
        cancelled,
        serde_json::json!({"job_id": id, "status": "cancelled"})
    );

    let requeued = json(&home, &["job", "retry", &id], 0);
    assert_eq!(
        requeued,
        serde_json::json!({"job_id": id, "status": "queued"})
    );

    let (code, stdout, _) = run(&home, &["job", "retry", &id]);
    assert_eq!(code, 3, "{stdout}");
    let error: Value = serde_json::from_str(&stdout).expect("a JSON error");
    assert_eq!(error["error"]["code"], "conflict");

    let (code, stdout, _) = run(
        &home,
        &["job", "cancel", "00000000-0000-0000-0000-000000000000"],
    );
    assert_eq!(code, 4, "{stdout}");
    let error: Value = serde_json::from_str(&stdout).expect("a JSON error");
    assert_eq!(error["error"]["code"], "not_found");
}

// covers: C102
#[test]
fn an_unknown_job_kind_is_a_usage_error() {
    let home = temp_home("unknown-kind");
    let (code, stdout, _) = run(&home, &["job", "enqueue", "bogus"]);
    assert_eq!(code, 2, "{stdout}");
    let error: Value = serde_json::from_str(&stdout).expect("a JSON error");
    assert_eq!(error["error"]["code"], "cli_error");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|text| text.starts_with("unknown job kind 'bogus'; known: agent_run,")),
        "{stdout}"
    );
}

// covers: C103
#[test]
fn job_list_pages_and_filters() {
    let home = temp_home("paging");
    for _ in 0..3 {
        enqueue(&home, "noop", "{}");
    }
    let page = json(&home, &["job", "list", "--limit", "2"], 0);
    assert_eq!(page["total"], 3);
    assert_eq!(page["items"].as_array().map(Vec::len), Some(2));

    let rest = json(&home, &["job", "list", "--offset", "2"], 0);
    assert_eq!(rest["items"].as_array().map(Vec::len), Some(1));

    let none = json(&home, &["job", "list", "--status", "bogus"], 0);
    assert_eq!(none["total"], 0);

    let matched = json(
        &home,
        &["job", "list", "--status", "queued", "--kind", "noop"],
        0,
    );
    assert_eq!(matched["total"], 3);
}

// covers: C101, C178
#[test]
fn compact_is_a_dry_run_by_default_and_recover_reports_its_passes() {
    let home = temp_home("maintenance");
    let dry = json(&home, &["job", "compact"], 0);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["hook_payloads"], 0);

    let applied = json(&home, &["job", "compact", "--apply"], 0);
    assert_eq!(applied["dry_run"], false);

    let recovered = json(&home, &["worker", "recover"], 0);
    assert_eq!(recovered["recovered_jobs"], serde_json::json!([]));
    assert_eq!(recovered["lost_workers"], serde_json::json!([]));
    assert_eq!(recovered["worktrees"], "not ported");
}
