//! IF-50: the `claude hook` fast path. Checks run the hook against a real home (spool, ingestion and
//! the debug line) and the real binary (dispatch before clap, empty output, exit 0). `covers:` markers
//! name the contract ids the checks exercise.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use wg_cli::hook::process;
use wg_runtime::{App, resolve_paths};
use wg_telemetry::ingest::HookEnv;
use wg_telemetry::spool::MAX_INPUT_BYTES;
use wg_telemetry_feature::service::hooks::hook_stats;

type TestResult = Result<(), Box<dyn Error>>;

const RECEIVED_AT: &str = "2026-10-26T12:00:00.000001+00:00";

/// A fresh home directory for one check.
fn fresh_home(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if50")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn no_env() -> HookEnv {
    HookEnv {
        attempt_id: None,
        task_id: None,
        workspace: None,
    }
}

/// The spooled record files of a home.
fn spool_files(home: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let dir = PathBuf::from(resolve_paths(Some(home))?.spool_dir().as_str());
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    Ok(files)
}

fn read_spooled(path: &Path) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// Creates the database at `home` (what `workgraph init` does) and returns its hook statistics.
fn stats_of(home: &Path) -> Result<Value, Box<dyn Error>> {
    let mut app = App::open(Some(home))?;
    Ok(hook_stats(&mut app)?)
}

// covers: P01
// covers: F09
#[test]
fn invalid_json_without_a_database_is_spooled_as_invalid_and_exits_zero() -> TestResult {
    let home = fresh_home("invalid")?;
    let mut stderr = Vec::new();
    let code = process(
        Some(&home),
        RECEIVED_AT,
        &no_env(),
        b"not json at all",
        false,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert!(stderr.is_empty(), "no diagnostics without --debug");

    let files = spool_files(&home)?;
    assert_eq!(files.len(), 1);
    let record = read_spooled(&files[0])?;
    assert_eq!(record["received_at"], json!(RECEIVED_AT));
    assert_eq!(
        record["env"],
        json!({"attempt_id": null, "task_id": null, "workspace": null})
    );
    assert_eq!(record["payload"]["hook_event_name"], json!("Invalid"));
    assert_eq!(record["payload"]["excerpt"], json!("not json at all"));
    assert_eq!(
        record["payload"]["error"],
        json!("Expecting value: line 1 column 1 (char 0)")
    );
    assert_eq!(
        record["spool_reason"],
        json!("RuntimeError: Workgraph database not initialised")
    );
    Ok(())
}

// covers: O15
#[test]
fn debug_reports_the_spooled_failure_on_stderr_only() -> TestResult {
    let home = fresh_home("debug")?;
    let mut stderr = Vec::new();
    let code = process(
        Some(&home),
        RECEIVED_AT,
        &no_env(),
        br#"{"hook_event_name": "Stop"}"#,
        true,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert_eq!(
        String::from_utf8(stderr)?,
        "workgraph hook spooled: Workgraph database not initialised\n"
    );
    assert_eq!(spool_files(&home)?.len(), 1);
    Ok(())
}

// covers: O15
#[test]
fn empty_input_is_the_empty_object_and_oversized_input_keeps_its_size() -> TestResult {
    let home = fresh_home("shape")?;
    let mut stderr = Vec::new();
    process(Some(&home), RECEIVED_AT, &no_env(), b"", false, &mut stderr);
    let oversized = vec![b' '; MAX_INPUT_BYTES + 1];
    process(
        Some(&home),
        RECEIVED_AT,
        &no_env(),
        &oversized,
        false,
        &mut stderr,
    );

    let files = spool_files(&home)?;
    assert_eq!(files.len(), 2);
    let payloads: Vec<Value> = files
        .iter()
        .map(|file| read_spooled(file).map(|record| record["payload"].clone()))
        .collect::<Result<_, _>>()?;
    assert!(payloads.contains(&json!({})));
    assert!(
        payloads.contains(&json!({"hook_event_name": "Oversized", "size": MAX_INPUT_BYTES + 1}))
    );
    Ok(())
}

// covers: C050
// covers: F09
#[test]
fn the_launch_context_is_recorded_as_read_from_the_environment() -> TestResult {
    let home = fresh_home("env")?;
    let env = HookEnv {
        attempt_id: Some("att".to_owned()),
        task_id: Some(String::new()),
        workspace: Some("main".to_owned()),
    };
    let mut stderr = Vec::new();
    process(Some(&home), RECEIVED_AT, &env, b"{}", false, &mut stderr);
    let files = spool_files(&home)?;
    assert_eq!(files.len(), 1);
    let record = read_spooled(&files[0])?;
    assert_eq!(
        record["env"],
        json!({"attempt_id": "att", "task_id": "", "workspace": "main"})
    );
    Ok(())
}

// covers: P01
#[test]
fn an_ingested_payload_leaves_no_spool_file() -> TestResult {
    let home = fresh_home("ingest")?;
    drop(App::open(Some(&home))?);
    let payload = json!({
        "hook_event_name": "SessionStart",
        "session_id": "sess-if50",
        "cwd": "C:/dev/none",
        "source": "startup",
    });
    let mut stderr = Vec::new();
    let code = process(
        Some(&home),
        RECEIVED_AT,
        &no_env(),
        payload.to_string().as_bytes(),
        true,
        &mut stderr,
    );
    assert_eq!(code, 0);
    assert!(stderr.is_empty(), "an ingested record reports nothing");
    assert!(spool_files(&home)?.is_empty());

    let stats = stats_of(&home)?;
    assert_eq!(stats["hook_events"], json!(1));
    assert_eq!(stats["by_event"], json!({"SessionStart": 1}));
    assert_eq!(stats["spooled"], json!(0));
    Ok(())
}

/// The real binary: `claude hook` is answered before the command line is parsed, so it prints nothing,
/// exits 0 and spools what it cannot ingest, even for arguments clap would reject.
// covers: P01
// covers: O15
#[test]
fn the_binary_answers_claude_hook_silently_with_exit_zero() -> TestResult {
    let home = fresh_home("binary")?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .args(["claude", "hook", "--unknown-flag"])
        .env("WORKGRAPH_HOME", &home)
        .env("WORKGRAPH_TASK_ID", "task-if50")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(b"{\"hook_event_name\": \"Stop\", \"session_id\": \"x\"")?;
    let output = child.wait_with_output()?;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());

    let files = spool_files(&home)?;
    assert_eq!(files.len(), 1);
    let record = read_spooled(&files[0])?;
    assert_eq!(record["env"]["task_id"], json!("task-if50"));
    assert_eq!(record["payload"]["hook_event_name"], json!("Invalid"));
    Ok(())
}
