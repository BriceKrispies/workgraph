//! `claude install` and `claude uninstall` (C051, C057) over temporary homes and project
//! directories. Every check passes its home, `CLAUDE_CONFIG_DIR` and command explicitly, so the
//! user's own Claude Code settings and `PATH` lookups are never touched.

use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use wg_telemetry_feature::cli::GROUP;
use wg_telemetry_feature::service::claude::{self, ClaudeEnv};
use wg_telemetry_feature::spec::installer::DEFAULT_EVENTS;
use wg_types::ErrorCode;

type TestResult = Result<(), Box<dyn Error>>;

const COMMAND: &str = "/opt/wg/workgraph";

/// A fresh directory under the test target's temporary area, empty.
fn fresh_dir(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("claude_install")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The environment of a temporary home with `CLAUDE_CONFIG_DIR` unset.
fn env_in(home: &Path) -> ClaudeEnv {
    ClaudeEnv {
        home: home.to_path_buf(),
        config_dir: None,
    }
}

/// The user settings file of a temporary home.
fn user_settings(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

/// The backup files beside `settings`, sorted by name.
fn backups_of(settings: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let dir = settings.parent().ok_or("no parent")?;
    let prefix = format!(
        "{}.workgraph-backup-",
        settings.file_name().ok_or("no name")?.to_string_lossy()
    );
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
        })
        .collect();
    found.sort();
    Ok(found)
}

// covers: C051
#[test]
fn install_writes_every_default_event_into_a_new_file() -> TestResult {
    let home = fresh_dir("fresh")?;
    let out = claude::install(&env_in(&home), "user", None, &[], Some(COMMAND), false)?;
    let settings = user_settings(&home);
    assert_eq!(out["settings"], json!(settings.display().to_string()));
    assert_eq!(out["added"], json!(DEFAULT_EVENTS));
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["command"], json!(COMMAND));
    let text = std::fs::read_to_string(&settings)?;
    assert!(text.starts_with("{\n  \"hooks\": {\n    \"SessionStart\": [\n      {\n"));
    assert!(text.ends_with("}\n"), "{text}");
    let data: Value = serde_json::from_str(&text)?;
    let stop = &data["hooks"]["Stop"][0]["hooks"][0];
    assert_eq!(stop["command"], json!(COMMAND));
    assert_eq!(stop["args"], json!(["claude", "hook"]));
    assert_eq!(stop["timeout"], json!(10));
    assert_eq!(stop["async"], json!(true));
    assert_eq!(data["hooks"]["Stop"][0]["matcher"], json!(""));
    Ok(())
}

// covers: C051
#[test]
fn a_second_install_adds_nothing_and_leaves_the_file_alone() -> TestResult {
    let home = fresh_dir("twice")?;
    let env = env_in(&home);
    claude::install(&env, "user", None, &[], Some(COMMAND), false)?;
    let settings = user_settings(&home);
    let before = std::fs::read(&settings)?;
    let out = claude::install(&env, "user", None, &[], Some(COMMAND), false)?;
    assert_eq!(out["added"], json!([]));
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(std::fs::read(&settings)?, before);
    assert!(backups_of(&settings)?.is_empty());
    Ok(())
}

// covers: C051
#[test]
fn an_existing_file_keeps_its_settings_and_is_backed_up_first() -> TestResult {
    let home = fresh_dir("existing")?;
    let settings = user_settings(&home);
    std::fs::create_dir_all(settings.parent().ok_or("no parent")?)?;
    let original = "{\n  \"theme\": \"dark\",\n  \"hooks\": {\n    \"PreToolUse\": [\n      {\"hooks\": [{\"type\": \"command\", \"command\": \"/usr/bin/other\"}]}\n    ]\n  }\n}\n";
    std::fs::write(&settings, original)?;
    let out = claude::install(
        &env_in(&home),
        "user",
        None,
        &["PreToolUse".to_owned(), "Stop".to_owned()],
        Some(COMMAND),
        false,
    )?;
    assert_eq!(out["added"], json!(["PreToolUse", "Stop"]));
    let backups = backups_of(&settings)?;
    assert_eq!(backups.len(), 1, "one backup beside the settings file");
    let backup_name = backups[0]
        .file_name()
        .ok_or("no name")?
        .to_string_lossy()
        .into_owned();
    let stamp = backup_name
        .strip_prefix("settings.json.workgraph-backup-")
        .ok_or("prefix")?;
    assert_eq!(stamp.len(), 16, "{stamp}");
    assert!(
        stamp.ends_with('Z') && stamp.as_bytes()[8] == b'T',
        "{stamp}"
    );
    assert_eq!(std::fs::read_to_string(&backups[0])?, original);
    assert_eq!(out["backup"], json!(backups[0].display().to_string()));
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&settings)?)?;
    assert_eq!(data["theme"], json!("dark"));
    let pre = data["hooks"]["PreToolUse"].as_array().ok_or("groups")?;
    assert_eq!(
        pre.len(),
        2,
        "the other handler stays and the Workgraph group follows"
    );
    assert_eq!(pre[0]["hooks"][0]["command"], json!("/usr/bin/other"));
    Ok(())
}

// covers: C051
#[test]
fn an_event_subset_installs_only_those_events() -> TestResult {
    let home = fresh_dir("subset")?;
    let out = claude::install(
        &env_in(&home),
        "user",
        None,
        &["Stop".to_owned()],
        Some(COMMAND),
        false,
    )?;
    assert_eq!(out["added"], json!(["Stop"]));
    let data: Value = serde_json::from_str(&std::fs::read_to_string(user_settings(&home))?)?;
    assert_eq!(data["hooks"].as_object().map(serde_json::Map::len), Some(1));
    Ok(())
}

// covers: C051
#[test]
fn a_settings_file_that_cannot_be_used_is_refused_untouched() -> TestResult {
    let home = fresh_dir("refused")?;
    let settings = user_settings(&home);
    std::fs::create_dir_all(settings.parent().ok_or("no parent")?)?;
    std::fs::write(&settings, "{\"a\" 1}")?;
    let error = claude::install(&env_in(&home), "user", None, &[], Some(COMMAND), false)
        .err()
        .ok_or("expected a refusal")?;
    assert!(matches!(error.code, ErrorCode::ConfigError));
    assert!(
        error.message.contains("refusing to modify it"),
        "{}",
        error.message
    );
    assert_eq!(std::fs::read_to_string(&settings)?, "{\"a\" 1}");
    assert!(backups_of(&settings)?.is_empty());

    std::fs::write(&settings, "{\"hooks\": []}")?;
    let error = claude::install(&env_in(&home), "user", None, &[], Some(COMMAND), false)
        .err()
        .ok_or("expected a refusal")?;
    assert_eq!(
        error.message,
        format!(
            "'hooks' in {} is not an object; refusing to modify it",
            settings.display()
        )
    );
    Ok(())
}

// covers: C057
#[test]
fn uninstall_removes_only_the_workgraph_handlers() -> TestResult {
    let home = fresh_dir("uninstall")?;
    let settings = user_settings(&home);
    std::fs::create_dir_all(settings.parent().ok_or("no parent")?)?;
    std::fs::write(
        &settings,
        "{\"theme\": \"dark\", \"hooks\": {\"PreToolUse\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"/usr/bin/other\"}]}]}}",
    )?;
    let env = env_in(&home);
    claude::install(&env, "user", None, &[], Some(COMMAND), false)?;
    let installed = std::fs::read_to_string(&settings)?;
    let out = claude::uninstall(&env, "user", None)?;
    let mut removed = DEFAULT_EVENTS.to_vec();
    removed.sort_unstable();
    assert_eq!(out["removed"], json!(removed));
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&settings)?)?;
    assert_eq!(data["theme"], json!("dark"));
    assert_eq!(
        data,
        json!({"theme": "dark", "hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "/usr/bin/other"}]}]}})
    );
    assert!(
        !backups_of(&settings)?.is_empty(),
        "the install backed the file up"
    );
    assert!(installed.contains(COMMAND));
    Ok(())
}

// covers: C057
#[test]
fn uninstall_drops_the_hooks_key_once_it_is_empty() -> TestResult {
    let home = fresh_dir("empty")?;
    let env = env_in(&home);
    claude::install(&env, "user", None, &[], Some(COMMAND), false)?;
    let out = claude::uninstall(&env, "user", None)?;
    let mut sorted = DEFAULT_EVENTS.to_vec();
    sorted.sort_unstable();
    assert_eq!(out["removed"], json!(sorted));
    assert_eq!(std::fs::read_to_string(user_settings(&home))?, "{}\n");
    Ok(())
}

// covers: C057
#[test]
fn uninstall_without_a_file_or_handlers_writes_nothing() -> TestResult {
    let home = fresh_dir("noop")?;
    let out = claude::uninstall(&env_in(&home), "user", None)?;
    assert_eq!(out["removed"], json!([]));
    assert_eq!(out["backup"], Value::Null);
    assert!(!user_settings(&home).exists());
    Ok(())
}

// covers: C051
#[test]
fn project_and_local_scopes_write_their_own_files() -> TestResult {
    let home = fresh_dir("scopes")?;
    let project = fresh_dir("scopes-project")?;
    let env = env_in(&home);
    claude::install(&env, "project", Some(&project), &[], Some(COMMAND), false)?;
    assert!(project.join(".claude").join("settings.json").exists());
    claude::install(&env, "local", Some(&project), &[], Some(COMMAND), false)?;
    assert!(project.join(".claude").join("settings.local.json").exists());
    assert!(!user_settings(&home).exists(), "the user file is untouched");
    let out = claude::uninstall(&env, "local", Some(&project))?;
    assert_eq!(
        out["removed"].as_array().map(Vec::len),
        Some(DEFAULT_EVENTS.len())
    );
    Ok(())
}

// covers: C051
#[test]
fn the_install_and_uninstall_parse_the_snapshot_options() {
    let install = (GROUP.command)().try_get_matches_from([
        "claude",
        "install",
        "--scope",
        "project",
        "--event",
        "Stop",
        "--event",
        "PreToolUse",
        "--command",
        COMMAND,
        "--mcp",
    ]);
    assert!(install.is_ok(), "{install:?}");
    assert!(
        (GROUP.command)()
            .try_get_matches_from(["claude", "install"])
            .is_ok()
    );
    let uninstall = (GROUP.command)().try_get_matches_from([
        "claude",
        "uninstall",
        "--scope",
        "local",
        "--project-dir",
        "/work/p",
    ]);
    assert!(uninstall.is_ok());
    let no_command =
        (GROUP.command)().try_get_matches_from(["claude", "uninstall", "--command", COMMAND]);
    assert!(no_command.is_err(), "uninstall takes no --command");
}
