//! Real-process and real-git tests for the demo agent and its checks, as the Python end-to-end
//! demonstration exercised them: every agent mode runs as a child process against real git
//! repositories created in a temporary directory.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_workgraph-demo-agent");

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh temporary directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("wg-demo-agent-{label}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Human")
        .env("GIT_AUTHOR_EMAIL", "human@test.local")
        .env("GIT_COMMITTER_NAME", "Test Human")
        .env("GIT_COMMITTER_EMAIL", "human@test.local")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository on `main` with one seed commit. `plan` becomes `workgraph-demo.json` when given.
fn make_repo(root: &Path, name: &str, plan: Option<(&str, &str)>) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "core.autocrlf", "false"]);
    fs::write(path.join("README.md"), format!("# {name}\n")).unwrap();
    if let Some((file, content)) = plan {
        let json = serde_json::json!({"file": file, "content": content}).to_string();
        fs::write(path.join("workgraph-demo.json"), json).unwrap();
    }
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "Initial"]);
    path
}

fn write_manifest(root: &Path, repos: &[(&Path, &str, &str)]) -> PathBuf {
    let entries: Vec<serde_json::Value> = repos
        .iter()
        .map(|(path, name, access)| {
            serde_json::json!({
                "repository": name,
                "checkout_path": path.display().to_string(),
                "access": access,
            })
        })
        .collect();
    let manifest = root.join("manifest.json");
    fs::write(
        &manifest,
        serde_json::json!({"repositories": entries}).to_string(),
    )
    .unwrap();
    manifest
}

fn run_agent(mode: &str, manifest: &Path, key: &str) -> Output {
    let mut child = Command::new(BIN)
        .args(["--mode", mode])
        .arg(manifest)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = serde_json::json!({"task": {"key": key}}).to_string();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn messages(output: &Output) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn run_check(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"])
}

const PLAN_CONTENT: &str = "# {task}\nWORKGRAPH_DEMO_OK = True\n";

#[test]
fn success_writes_the_planned_file_and_commits_with_trailer() {
    let tmp = TempDir::new("success");
    let repo = make_repo(
        tmp.path(),
        "auth-service",
        Some(("src/auth/token.py", PLAN_CONTENT)),
    );
    let manifest = write_manifest(tmp.path(), &[(&repo, "auth-service", "read_write")]);

    let output = run_agent("success", &manifest, "WG-7");
    assert_eq!(output.status.code(), Some(0));
    let msgs = messages(&output);
    assert_eq!(msgs[0]["type"], "progress");
    assert_eq!(msgs[0]["message"], "starting WG-7");
    assert_eq!(msgs[1]["percent"], 60);
    assert_eq!(msgs[2]["type"], "usage");
    assert_eq!(msgs[3]["status"], "succeeded");
    assert!(
        msgs[3]["summary"]
            .as_str()
            .unwrap()
            .starts_with("changed auth-service:")
    );

    let written = fs::read_to_string(repo.join("src/auth/token.py")).unwrap();
    assert_eq!(written, "# WG-7\nWORKGRAPH_DEMO_OK = True\n");
    let body = git(&repo, &["log", "-1", "--format=%B"]);
    assert!(body.lines().last().unwrap().ends_with("Task-ID: WG-7"));
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%an <%ae>"]),
        "Workgraph Scripted Agent <agent@workgraph.local>"
    );

    let check = run_check(&repo, &["check"]);
    assert_eq!(check.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&check.stdout).trim(), "1 passed");
}

#[test]
fn buggy_mode_commits_a_marker_the_contract_check_rejects() {
    let tmp = TempDir::new("buggy");
    let repo = make_repo(
        tmp.path(),
        "shared-sdk",
        Some(("sdk/client.py", PLAN_CONTENT)),
    );
    let manifest = write_manifest(tmp.path(), &[(&repo, "shared-sdk", "read_write")]);

    let output = run_agent("buggy", &manifest, "WG-8");
    assert_eq!(output.status.code(), Some(0));
    assert!(
        fs::read_to_string(repo.join("sdk/client.py"))
            .unwrap()
            .contains("TODO_FINISH")
    );

    let check = run_check(&repo, &["check"]);
    assert_eq!(check.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("test_client_contract - marker missing")
    );
}

#[test]
fn contract_check_reports_a_missing_file() {
    let tmp = TempDir::new("missing");
    let repo = make_repo(
        tmp.path(),
        "web-client",
        Some(("web/theme.ts", PLAN_CONTENT)),
    );
    let check = run_check(&repo, &["check"]);
    assert_eq!(check.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&check.stdout).contains("test_theme_exists - missing web"));
}

#[test]
fn without_a_plan_file_the_agent_appends_a_function_each_run() {
    let tmp = TempDir::new("append");
    let repo = make_repo(tmp.path(), "plain", None);
    let manifest = write_manifest(tmp.path(), &[(&repo, "plain", "read_write")]);

    assert_eq!(
        run_agent("success", &manifest, "WG-1").status.code(),
        Some(0)
    );
    assert_eq!(
        run_agent("success", &manifest, "WG-2").status.code(),
        Some(0)
    );
    let text = fs::read_to_string(repo.join("agent_change.py")).unwrap();
    assert_eq!(
        text,
        "def change_0():\n    return 'WG-1'\ndef change_2():\n    return 'WG-2'\n"
    );
}

#[test]
fn fail_mode_reports_failure_and_commits_nothing() {
    let tmp = TempDir::new("fail");
    let repo = make_repo(tmp.path(), "auth-service", None);
    let manifest = write_manifest(tmp.path(), &[(&repo, "auth-service", "read_write")]);
    let before = head(&repo);

    let output = run_agent("fail", &manifest, "WG-3");
    assert_eq!(output.status.code(), Some(1));
    let msgs = messages(&output);
    assert_eq!(msgs.last().unwrap()["status"], "failed");
    assert_eq!(msgs.last().unwrap()["error"], "simulated failure");
    assert_eq!(head(&repo), before);
}

#[test]
fn crash_mode_exits_three_with_unstructured_output() {
    let tmp = TempDir::new("crash");
    let repo = make_repo(tmp.path(), "auth-service", None);
    let manifest = write_manifest(tmp.path(), &[(&repo, "auth-service", "read_write")]);

    let output = run_agent("crash", &manifest, "WG-4");
    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.ends_with("boom: simulated crash\n"), "{stdout}");
    assert!(!stdout.contains("\"type\":\"result\""));
}

#[test]
fn noop_mode_succeeds_without_a_commit() {
    let tmp = TempDir::new("noop");
    let repo = make_repo(tmp.path(), "auth-service", None);
    let manifest = write_manifest(tmp.path(), &[(&repo, "auth-service", "read_write")]);
    let before = head(&repo);

    let output = run_agent("noop", &manifest, "WG-5");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        messages(&output).last().unwrap()["summary"],
        "changed nothing"
    );
    assert_eq!(head(&repo), before);
}

#[test]
fn read_only_repositories_are_skipped_unless_touch_ro() {
    let tmp = TempDir::new("access");
    let writable = make_repo(tmp.path(), "writable", None);
    let readonly = make_repo(tmp.path(), "readonly", None);
    let manifest = write_manifest(
        tmp.path(),
        &[
            (&writable, "writable", "read_write"),
            (&readonly, "readonly", "read_only"),
        ],
    );
    let readonly_before = head(&readonly);

    let output = run_agent("success", &manifest, "WG-6");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(head(&readonly), readonly_before);
    assert!(
        !messages(&output).last().unwrap()["summary"]
            .as_str()
            .unwrap()
            .contains("readonly")
    );

    let touched = run_agent("touch-ro", &manifest, "WG-6");
    assert_eq!(touched.status.code(), Some(0));
    assert_ne!(head(&readonly), readonly_before);
}

#[test]
fn missing_manifest_is_a_usage_error() {
    let output = Command::new(BIN)
        .arg("--mode")
        .arg("success")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn integration_check_reports_missing_repository_paths() {
    let tmp = TempDir::new("integration");
    let present = tmp.path().join("present");
    fs::create_dir_all(&present).unwrap();
    let ok = Command::new(BIN)
        .arg("integration")
        .env("WORKGRAPH_REPO_AUTH_SERVICE_PATH", &present)
        .env_remove("WORKGRAPH_REPO_SHARED_SDK_PATH")
        .env_remove("WORKGRAPH_REPO_WEB_CLIENT_PATH")
        .output()
        .unwrap();
    assert_eq!(ok.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&ok.stdout).trim(), "integration ok");

    let bad = Command::new(BIN)
        .arg("integration")
        .env("WORKGRAPH_REPO_SHARED_SDK_PATH", tmp.path().join("absent"))
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&bad.stdout).trim(),
        "missing ['SHARED_SDK']"
    );
}
