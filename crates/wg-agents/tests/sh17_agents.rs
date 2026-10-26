//! Real-process tests for the Claude Code adapter (P02) and the command adapter (P04), as the
//! Python tests ran them: the fake `claude` (`wg-fake-claude`, the Rust form of
//! `tests/fixtures/fake_claude.py`) and the scripted agent (`workgraph-demo-agent`) run as child
//! processes through `wg-proc`, against real git repositories in a temporary directory.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use wg_agents::{
    AgentError, AgentEvent, AgentRunRequest, AgentRunResult, AgentRunner, ClaudeCodeProvider,
    ClaudeConfig, CommandAgentProvider, CommandConfig, RunStatus,
};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_wg-fake-claude");

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("wg-agents-{label}-{}-{n}", std::process::id()));
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
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn make_repo(root: &Path, name: &str, plan: Option<(&str, &str)>) -> PathBuf {
    let path = root.join(name);
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "core.autocrlf", "false"]);
    fs::write(path.join("README.md"), format!("# {name}\n")).unwrap();
    if let Some((file, content)) = plan {
        let plan = json!({"file": file, "content": content}).to_string();
        fs::write(path.join("workgraph-demo.json"), plan).unwrap();
    }
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "Initial"]);
    path
}

/// The events a run streamed.
type Events = Arc<Mutex<Vec<AgentEvent>>>;

fn collector() -> (Events, impl FnMut(AgentEvent) + Send + 'static) {
    let events: Events = Arc::default();
    let sink = Arc::clone(&events);
    (events, move |event| sink.lock().unwrap().push(event))
}

fn kinds(events: &Events) -> Vec<&'static str> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.kind.as_str())
        .collect()
}

fn request(dir: &Path, workdir: &Path, prompt: &str) -> AgentRunRequest {
    let manifest_path = dir.join("workgraph-manifest.json");
    AgentRunRequest {
        attempt_id: "att-1".into(),
        task_id: "task-id-1".into(),
        task_key: "WG-7".into(),
        prompt: prompt.into(),
        workdir: workdir.to_path_buf(),
        manifest_path,
        manifest: json!({}),
        timeout_seconds: 60.0,
        log_path: dir.join("logs").join("attempt.log"),
        env: Vec::new(),
        model: None,
        resume_session_id: None,
        additional_dirs: Vec::new(),
    }
}

fn claude(model: Option<&str>) -> ClaudeCodeProvider {
    ClaudeCodeProvider::new(
        "claude_code",
        ClaudeConfig {
            executable: FAKE_CLAUDE.to_owned(),
            model: model.map(str::to_owned),
            allowed_tools: vec!["Edit".into()],
            ..ClaudeConfig::default()
        },
    )
}

// covers: P02
#[test]
fn claude_stream_json_success_commits_and_reports_telemetry() {
    let tmp = TempDir::new("claude-ok");
    let repo = make_repo(tmp.path(), "cc", None);
    let provider = claude(Some("claude-test"));
    let (ok, detail) = provider.check_available();
    assert!(ok && detail.contains("Claude Code"), "{detail}");
    assert!(provider.capabilities().usage_reporting && provider.capabilities().lifecycle_hooks);

    let (events, sink) = collector();
    let req = request(tmp.path(), &repo, "Do it. Commit with `Task-ID: WG-7`");
    let result = provider.run(&req, sink, None).unwrap();

    assert_eq!(result.status, RunStatus::Succeeded, "{:?}", result.error);
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.summary, "All done");
    assert_eq!(result.usage.get("input_tokens"), Some(&json!(900)));
    assert_eq!(result.usage.get("num_turns"), Some(&json!(3)));
    assert_eq!(result.cost_usd, Some(0.0123));
    assert_eq!(result.provider_duration_ms, Some(4321));
    assert_eq!(result.model.as_deref(), Some("claude-test"));
    // The session id was fixed up front and echoed back by the fake.
    let session = result.session_id.clone().unwrap();
    assert_eq!(session.len(), 36);
    assert_eq!(result.telemetry["tool_uses"], 2);
    assert_eq!(result.telemetry["tool_errors"], 1);
    assert_eq!(result.telemetry["subagent_launches"], 1);
    assert_eq!(result.telemetry["unparsed_lines"], 1);
    assert_eq!(result.telemetry["tools"], json!({"Edit": 1, "Task": 1}));
    assert_eq!(
        kinds(&events),
        [
            "session",
            "message",
            "tool_use",
            "tool_use",
            "tool_result",
            "log",
            "result"
        ]
    );
    let log = fs::read_to_string(&req.log_path).unwrap();
    assert!(log.contains("\"type\":\"result\"") && log.contains("a stray line"));
    // The fake committed with the trailer, as the Python test counted task commits.
    let body = git(&repo, &["log", "-1", "--format=%B"]);
    assert!(body.ends_with("Task-ID: WG-7"), "{body}");
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]), "2");
}

// covers: P02
#[test]
fn claude_failure_and_resume() {
    let tmp = TempDir::new("claude-err");
    let repo = make_repo(tmp.path(), "cc", None);
    let provider = claude(None);

    let mut req = request(tmp.path(), &repo, "no trailer here");
    req.env = vec![("FAKE_CLAUDE_MODE".into(), "error".into())];
    let (_, sink) = collector();
    let failed = provider.run(&req, sink, None).unwrap();
    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(failed.exit_code, Some(1));
    assert_eq!(failed.error.as_deref(), Some("All done"));

    // A resumed session passes `--resume`, so the fake reports its fixed "resumed" id.
    let mut resumed = request(tmp.path(), &repo, "again");
    resumed.resume_session_id = Some("old-session".into());
    let (_, sink) = collector();
    let result = provider.run(&resumed, sink, None).unwrap();
    assert_eq!(result.session_id.as_deref(), Some("resumed"));
}

// covers: P02
#[test]
fn claude_timeout_and_cancellation_kill_the_tree() {
    let tmp = TempDir::new("claude-sleep");
    let repo = make_repo(tmp.path(), "cc", None);
    let provider = claude(None);

    let mut req = request(tmp.path(), &repo, "sleep");
    req.env = vec![("FAKE_CLAUDE_MODE".into(), "sleep".into())];
    req.timeout_seconds = 1.5;
    let (_, sink) = collector();
    let started = Instant::now();
    let timed_out = provider.run(&req, sink, None).unwrap();
    assert_eq!(timed_out.status, RunStatus::TimedOut);
    assert_eq!(timed_out.error.as_deref(), Some("Claude Code timed out"));
    assert!(started.elapsed() < Duration::from_secs(30));

    req.timeout_seconds = 60.0;
    let begun = Instant::now();
    let cancel = move || begun.elapsed() > Duration::from_millis(1500);
    let (_, sink) = collector();
    let cancelled = provider.run(&req, sink, Some(&cancel)).unwrap();
    assert_eq!(cancelled.status, RunStatus::Cancelled);
    assert_eq!(cancelled.error.as_deref(), Some("cancelled"));
}

// covers: P02
#[test]
fn claude_unavailable() {
    let tmp = TempDir::new("claude-missing");
    let provider = ClaudeCodeProvider::new(
        "claude_code",
        ClaudeConfig {
            executable: "claude-not-installed-anywhere".into(),
            ..ClaudeConfig::default()
        },
    );
    let (ok, detail) = provider.check_available();
    assert!(!ok && detail.contains("install Claude Code"), "{detail}");
    assert!(detail.contains("was not found on PATH"));
    let (_, sink) = collector();
    let error = provider
        .run(&request(tmp.path(), tmp.path(), "x"), sink, None)
        .unwrap_err();
    assert!(
        matches!(&error, AgentError::Unavailable(m) if m.starts_with("Claude Code is unavailable"))
    );

    let snapshot = provider.config_snapshot();
    assert_eq!(snapshot["adapter"], "claude_code");
    assert_eq!(snapshot["permission_mode"], "acceptEdits");
}

/// The scripted agent binary, built on first use: another package's binary is not part of this
/// crate's test build.
fn demo_agent() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap();
    let name = format!("workgraph-demo-agent{}", std::env::consts::EXE_SUFFIX);
    let path = dir.join(&name);
    if !path.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let status = Command::new(cargo)
            .args(["build", "-q", "-p", "wg-demo-agent"])
            .status()
            .unwrap();
        assert!(status.success());
    }
    assert!(path.exists(), "{}", path.display());
    path
}

fn demo_provider(mode: &str, supports_usage: bool) -> CommandAgentProvider {
    let mut config = CommandConfig::new(demo_agent().display().to_string());
    config.args = ["--mode", mode, "{manifest}"].map(str::to_owned).to_vec();
    config.supports_usage = supports_usage;
    CommandAgentProvider::new("scripted", config).unwrap()
}

fn demo_request(tmp: &Path, repo: &Path) -> AgentRunRequest {
    let attempt_dir = tmp.join("attempt");
    fs::create_dir_all(&attempt_dir).unwrap();
    let manifest = json!({"repositories": [{
        "repository": "svc",
        "checkout_path": repo.display().to_string(),
        "access": "read_write",
    }]});
    let mut req = request(&attempt_dir, repo, "# Task WG-7: do it");
    fs::write(&req.manifest_path, manifest.to_string()).unwrap();
    req.manifest = manifest;
    req
}

// covers: P04
#[test]
fn command_agent_protocol_success() {
    let tmp = TempDir::new("cmd-ok");
    let repo = make_repo(
        tmp.path(),
        "svc",
        Some(("src/token.py", "# {task}\nWORKGRAPH_DEMO_OK = True\n")),
    );
    let provider = demo_provider("success", true);
    assert!(provider.check_available().0);
    let caps = provider.capabilities();
    assert!(caps.streaming_progress && caps.usage_reporting && !caps.session_resume);

    let req = demo_request(tmp.path(), &repo);
    let (events, sink) = collector();
    let result = provider.run(&req, sink, None).unwrap();

    assert_eq!(result.status, RunStatus::Succeeded, "{:?}", result.error);
    assert!(
        result.summary.starts_with("changed svc:"),
        "{}",
        result.summary
    );
    assert_eq!(result.usage.get("input_tokens"), Some(&json!(1200)));
    assert_eq!(result.usage.get("output_tokens"), Some(&json!(340)));
    assert_eq!(kinds(&events), ["progress", "progress", "usage", "result"]);
    assert_eq!(result.telemetry["progress_events"], 2);
    assert_eq!(result.telemetry["output_lines"], 4);
    // The agent really edited and committed in the checkout.
    assert_eq!(
        fs::read_to_string(repo.join("src/token.py")).unwrap(),
        "# WG-7\nWORKGRAPH_DEMO_OK = True\n"
    );
    assert!(git(&repo, &["log", "-1", "--format=%B"]).ends_with("Task-ID: WG-7"));
    // The protocol files sit beside the manifest, never in the repository.
    let dir = req.manifest_path.parent().unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("workgraph-prompt.md")).unwrap(),
        "# Task WG-7: do it"
    );
    let sent: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("workgraph-request.json")).unwrap())
            .unwrap();
    assert_eq!(sent["protocol"], "workgraph.agent.v1");
    assert_eq!(sent["task"], json!({"id": "task-id-1", "key": "WG-7"}));
    assert_eq!(sent["manifest"]["repositories"][0]["repository"], "svc");
    assert!(!repo.join("workgraph-prompt.md").exists());
}

// covers: P04
#[test]
fn command_agent_declared_failure_and_crash() {
    let tmp = TempDir::new("cmd-fail");
    let repo = make_repo(tmp.path(), "svc", None);
    let req = demo_request(tmp.path(), &repo);

    let (_, sink) = collector();
    let failed = demo_provider("fail", false).run(&req, sink, None).unwrap();
    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(failed.exit_code, Some(1));
    assert_eq!(failed.error.as_deref(), Some("simulated failure"));
    assert_eq!(failed.summary, "could not make it work");

    let (events, sink) = collector();
    let crashed = demo_provider("crash", false).run(&req, sink, None).unwrap();
    assert_eq!(crashed.status, RunStatus::Failed);
    assert_eq!(crashed.exit_code, Some(3));
    assert!(crashed.error.unwrap().contains("boom: simulated crash"));
    assert_eq!(kinds(&events), ["progress", "log"]);
}

// covers: P04
#[test]
fn command_agent_timeout_cancel_and_template() {
    let tmp = TempDir::new("cmd-sleep");
    let repo = make_repo(tmp.path(), "svc", None);
    let provider = demo_provider("sleep", false);
    let mut req = demo_request(tmp.path(), &repo);
    req.timeout_seconds = 1.5;
    let (_, sink) = collector();
    let timed_out: AgentRunResult = provider.run(&req, sink, None).unwrap();
    assert_eq!(timed_out.status, RunStatus::TimedOut);
    assert_eq!(timed_out.error.as_deref(), Some("agent timed out"));

    req.timeout_seconds = 60.0;
    let begun = Instant::now();
    let cancel = move || begun.elapsed() > Duration::from_millis(1500);
    let (_, sink) = collector();
    let cancelled = provider.run(&req, sink, Some(&cancel)).unwrap();
    assert_eq!(cancelled.status, RunStatus::Cancelled);

    let mut bad = CommandConfig::new("agent");
    bad.args = vec!["{manifest}".into(), "{nope}".into()];
    let error = CommandAgentProvider::new("bad", bad).unwrap_err();
    assert!(error.to_string().contains("unknown placeholder {nope}"));

    let missing =
        CommandAgentProvider::new("m", CommandConfig::new("no-such-agent-binary")).unwrap();
    let (ok, detail) = missing.check_available();
    assert!(!ok && detail.contains("was not found on PATH"));
    let snapshot = provider.config_snapshot();
    assert_eq!(snapshot["adapter"], "command");
    assert_eq!(snapshot["env_keys"], json!([]));
}
