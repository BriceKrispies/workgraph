//! The `claude` commands (C047 to C049, C052 to C056) and the status route (H121) over temporary
//! homes. Every check passes its home and `CLAUDE_CONFIG_DIR` explicitly, so the user's own Claude
//! Code settings are never read or written.

use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use wg_render::Console;
use wg_runtime::App;
use wg_telemetry_feature::cli::GROUP;
use wg_telemetry_feature::http;
use wg_telemetry_feature::service::claude::{self, ClaudeEnv};
use wg_telemetry_feature::spec::{installer, view};

type TestResult = Result<(), Box<dyn Error>>;

/// A fresh directory under the test target's temporary area, empty.
fn fresh_dir(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("claude_cli")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The environment of a temporary home: `home` for `Path.home()`, `CLAUDE_CONFIG_DIR` unset.
fn env_in(home: &Path) -> ClaudeEnv {
    ClaudeEnv {
        home: home.to_path_buf(),
        config_dir: None,
    }
}

/// The hook settings of an installed Workgraph: `Stop` runs the given executable in exec form.
fn settings_with_stop(exe: &Path) -> Value {
    json!({
        "theme": "dark",
        "hooks": {
            "Stop": [{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": exe.to_string_lossy(),
                    "args": ["claude", "hook"],
                    "timeout": 10,
                    "async": true
                }]
            }],
            "PreToolUse": [{"hooks": [{"type": "command", "command": "/usr/bin/other"}]}]
        }
    })
}

// covers: C055
#[test]
fn the_claude_group_is_the_snapshot_tree() -> TestResult {
    let command = (GROUP.command)();
    let names: Vec<&str> = command
        .get_subcommands()
        .map(|sub| sub.get_name())
        .collect();
    for expected in [
        "backfill-time",
        "backfill-transcripts",
        "bind",
        "mcp-config",
        "replay",
        "sessions",
        "status",
        "unbind",
    ] {
        assert!(names.contains(&expected), "missing {expected}");
    }
    let parsed = (GROUP.command)().try_get_matches_from(["claude", "status", "--scope", "local"]);
    assert!(parsed.is_ok());
    let missing_task = (GROUP.command)().try_get_matches_from(["claude", "bind"]);
    assert!(missing_task.is_err(), "bind needs its TASK argument");
    Ok(())
}

// covers: C055
#[test]
fn the_installation_reads_the_user_settings_of_the_temporary_home() -> TestResult {
    let home = fresh_dir("installation")?;
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;
    let exe = home.join("workgraph.exe");
    std::fs::write(&exe, b"")?;
    std::fs::write(
        claude_dir.join("settings.json"),
        serde_json::to_string_pretty(&settings_with_stop(&exe))?,
    )?;
    let status = claude::installation(&env_in(&home), "user", None);
    assert!(status.exists);
    assert_eq!(status.error, None);
    assert_eq!(status.installed_events, ["Stop"]);
    assert_eq!(status.missing_events.len(), 9);
    assert!(!status.missing_events.contains(&"Stop".to_owned()));
    assert!(status.command_resolves);
    assert_eq!(
        status.settings,
        claude_dir.join("settings.json").display().to_string()
    );
    Ok(())
}

// covers: C055
#[test]
fn a_refused_settings_file_is_reported_not_overwritten() -> TestResult {
    let home = fresh_dir("refused")?;
    let claude_dir = home.join(".claude");
    std::fs::create_dir_all(&claude_dir)?;
    let file = claude_dir.join("settings.json");
    std::fs::write(&file, "{\"a\" 1}")?;
    let status = claude::installation(&env_in(&home), "user", None);
    assert_eq!(
        status.error.as_deref(),
        Some(
            format!(
                "{} is not valid JSON (Expecting ':' delimiter: line 1 column 6 (char 5)); refusing to modify it",
                file.display()
            )
            .as_str()
        )
    );
    assert!(status.installed_events.is_empty());
    assert!(!status.command_resolves);
    assert_eq!(std::fs::read_to_string(&file)?, "{\"a\" 1}");
    Ok(())
}

// covers: C055
#[test]
fn the_config_dir_overrides_the_home_for_the_user_scope() -> TestResult {
    let home = fresh_dir("config-dir-home")?;
    let config = fresh_dir("config-dir")?;
    std::fs::write(config.join("settings.json"), "{}")?;
    let env = ClaudeEnv {
        home: home.clone(),
        config_dir: Some(config.clone()),
    };
    let status = claude::installation(&env, "user", None);
    assert!(status.exists);
    assert_eq!(
        status.settings,
        config.join("settings.json").display().to_string()
    );
    assert!(!status.command_resolves);
    Ok(())
}

// covers: C052
#[test]
fn the_mcp_block_is_the_stdio_launch_and_prints_as_indented_json() -> TestResult {
    let config = claude::mcp_config(Some("/opt/wg/workgraph"))?;
    assert_eq!(
        config,
        json!({"mcpServers": {"workgraph": {"type": "stdio", "command": "/opt/wg/workgraph", "args": ["mcp"]}}})
    );
    let printed = view::mcp_config_view(&Console::new(80), &config);
    assert_eq!(
        printed,
        "{\n  \"mcpServers\": {\n    \"workgraph\": {\n      \"type\": \"stdio\",\n      \"command\": \"/opt/wg/workgraph\",\n      \"args\": [\n        \"mcp\"\n      ]\n    }\n  }\n}\n"
    );
    assert_eq!(installer::mcp_server_config("/opt/wg/workgraph"), config);
    Ok(())
}

// covers: C055
// covers: H121
#[test]
fn the_status_document_and_the_route_share_the_shape() -> TestResult {
    let home = fresh_dir("status")?;
    let mut app = App::open(Some(home.join("wg").as_path()))?;
    let env = env_in(&home);
    let status = claude::status(&mut app, &env, "user", None)?;
    let keys: Vec<&str> = status
        .as_object()
        .map(|map| map.keys().map(String::as_str).collect())
        .unwrap_or_default();
    assert_eq!(keys, ["installation", "ingestion", "sessions"]);
    assert_eq!(status["installation"]["exists"], json!(false));
    assert_eq!(status["ingestion"]["hook_events"], json!(0));
    assert_eq!(status["sessions"], json!([]));

    let route = http::document(&mut app, &env)?;
    let route_text = wg_json::cli_json(&route).map_err(|err| format!("{err:?}"))?;
    let user_at = route_text.find("\"user\"");
    let hooks_at = route_text.find("\"hooks\"");
    let sessions_at = route_text.find("\"sessions\"");
    assert!(user_at < hooks_at && hooks_at < sessions_at, "{route_text}");
    assert!(
        route_text.contains("\"command_resolves\": false"),
        "{route_text}"
    );

    let shown = view::status_view(&Console::new(80), &status);
    assert!(shown.contains("installation"), "{shown}");
    assert!(shown.contains("nothing to show"), "{shown}");
    Ok(())
}

// covers: C047
// covers: C048
// covers: C053
// covers: C054
#[test]
fn replay_backfills_and_sessions_read_an_empty_store() -> TestResult {
    let home = fresh_dir("empty-store")?;
    let mut app = App::open(Some(home.join("wg").as_path()))?;
    assert_eq!(
        claude::replay(&mut app, 1000)?,
        json!({"ingested": 0, "duplicate": 0, "failed": 0})
    );
    assert_eq!(
        claude::replay(&mut app, -1)?,
        json!({"ingested": 0, "duplicate": 0, "failed": 0})
    );
    assert_eq!(
        claude::backfill_time(&mut app, None)?,
        json!({"parsed": 0, "missing": 0, "skipped": 0})
    );
    assert_eq!(
        claude::backfill_transcripts(&mut app, Some(10))?,
        json!({"parsed": 0, "missing": 0, "linked": 0, "attempts_created": 0})
    );
    assert!(claude::sessions(&mut app, 30)?.is_empty());
    Ok(())
}

// covers: C056
// covers: C049
#[test]
fn unbind_matches_nothing_for_an_unknown_id_or_directory() -> TestResult {
    let home = fresh_dir("unbind")?;
    let mut app = App::open(Some(home.join("wg").as_path()))?;
    assert_eq!(claude::unbind(&mut app, Some("not-a-binding"), None)?, 0);
    assert_eq!(claude::unbind(&mut app, None, Some(home.as_path()))?, 0);
    assert!(
        claude::bind(&mut app, None, "WG-1", None, Some(home.as_path())).is_err(),
        "a bind without a workspace has no task to attach to"
    );
    Ok(())
}
