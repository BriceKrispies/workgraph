//! IF-02: `init`, `doctor` and the `config` group, run through the real binary against a real
//! home directory.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if02-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn run_args(home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .arg("--home")
        .arg(home)
        .args(args)
        .env("COLUMNS", "120")
        .env("NO_COLOR", "1")
        .output()
        .expect("the binary runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn json_at(home: &Path, args: &[&str], expect: i32) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let (code, stdout, stderr) = run_args(home, &full);
    assert_eq!(code, expect, "{args:?}: {stdout}{stderr}");
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {stdout}"))
}

// covers: C099
#[test]
fn init_creates_storage_a_token_and_the_first_workspace() {
    let home = temp_home("init");
    let out = json_at(&home, &["init", "-w", "acme", "--prefix", "ac"], 0);
    assert_eq!(out["workspace_created"], "acme");
    assert_eq!(out["default_workspace"], "acme");
    assert_eq!(out["schema_revision"], "0011");
    assert!(home.join("config").join("config.toml").is_file());
    assert!(home.join("config").join("api-token").is_file());
    assert!(
        home.join("config")
            .join("workspaces")
            .join("acme.toml")
            .is_file()
    );
    // A second run changes nothing and says so.
    let again = json_at(&home, &["init", "-w", "acme", "--prefix", "AC"], 0);
    assert_eq!(again["workspace_created"], Value::Null);
    assert_eq!(again["default_workspace"], "acme");
}

// covers: C099
#[test]
fn init_without_a_workspace_and_with_a_bad_slug() {
    let home = temp_home("init-none");
    let out = json_at(&home, &["init", "--no-workspace"], 0);
    assert_eq!(out["workspace_created"], Value::Null);
    assert_eq!(out["default_workspace"], Value::Null);
    let bad = json_at(&home, &["init", "-w", "Bad_Slug"], 2);
    assert_eq!(bad["error"]["code"], "validation_failed");
    assert!(
        bad["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("slug must be lowercase letters, digits and dashes")
    );
}

// covers: C099
#[test]
fn init_titles_the_slug_for_the_name_unless_one_is_given() {
    let home = temp_home("init-name");
    json_at(&home, &["init", "-w", "my-team"], 0);
    json_at(&home, &["init", "-w", "other", "--name", "The Other"], 0);
    let (code, stdout, _) = run_args(&home, &["--json", "task", "list", "-w", "my-team"]);
    assert_eq!(code, 0, "{stdout}");
}

// covers: C061, C059
#[test]
fn config_show_and_path() {
    let home = temp_home("show");
    json_at(&home, &["init", "-w", "acme"], 0);
    let shown = json_at(&home, &["config", "show"], 0);
    assert_eq!(shown["workspace"], "acme");
    assert_eq!(shown["config"]["default_workspace"], "acme");
    assert!(
        shown["files"]["workspace"]
            .as_str()
            .expect("a path")
            .ends_with("acme.toml")
    );
    let paths = json_at(&home, &["config", "path"], 0);
    assert!(
        paths["database"]
            .as_str()
            .expect("a path")
            .ends_with("workgraph.db")
    );
    let (code, text, _) = run_args(&home, &["config", "path"]);
    assert_eq!(code, 0);
    assert!(text.contains("  config_file: "), "{text}");
}

// covers: C060, C062
#[test]
fn config_set_validates_before_it_writes() {
    let home = temp_home("set");
    json_at(&home, &["init", "-w", "acme"], 0);
    let set = json_at(&home, &["config", "set", "execution.enabled", "true"], 0);
    assert_eq!(
        set,
        json!({"key": "execution.enabled", "value": true, "workspace": null})
    );
    let shown = json_at(&home, &["config", "show"], 0);
    assert_eq!(shown["config"]["execution"]["enabled"], true);
    let bad = json_at(&home, &["config", "set", "execution.enabled", "maybe"], 7);
    assert_eq!(bad["error"]["code"], "cli_error");
    assert!(
        bad["error"]["message"]
            .as_str()
            .expect("a message")
            .starts_with("invalid value for execution.enabled: ")
    );
    let nested = json_at(&home, &["config", "set", "execution.enabled.x", "1"], 2);
    assert_eq!(
        nested["error"]["message"],
        "execution.enabled.x does not address a nested setting"
    );
    let valid = json_at(&home, &["config", "validate"], 0);
    assert_eq!(valid, json!({"valid": true, "problems": []}));
}

// covers: C060, C062, C058
#[test]
fn config_workspace_overrides_and_migrate() {
    let home = temp_home("override");
    json_at(&home, &["init", "-w", "acme"], 0);
    json_at(
        &home,
        &["config", "set", "-w", "acme", "execution.enabled", "true"],
        0,
    );
    let shown = json_at(&home, &["config", "show", "-w", "acme"], 0);
    assert_eq!(shown["config"]["execution"]["enabled"], true);
    let file = home.join("config").join("config.toml");
    std::fs::write(&file, "[agents]\nenabled = true\n").expect("an old file");
    let migrated = json_at(&home, &["config", "migrate"], 0);
    assert_eq!(migrated["rewritten"].as_array().map(Vec::len), Some(1));
    let text = std::fs::read_to_string(&file).expect("a file");
    assert!(text.contains("schema_version = 2"), "{text}");
    std::fs::write(&file, "schema_version = 2\nbogus = 1\n").expect("a bad file");
    let invalid = json_at(&home, &["config", "validate"], 7);
    assert_eq!(invalid["valid"], false);
}

// covers: C065
#[test]
fn doctor_reports_health() {
    let home = temp_home("doctor");
    json_at(&home, &["init"], 0);
    let report = json_at(&home, &["doctor"], 0);
    assert_eq!(report["healthy"], true);
    let names: Vec<&str> = report["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .filter_map(|check| check["check"].as_str())
        .collect();
    for wanted in [
        "python",
        "git",
        "schema",
        "database integrity",
        "configuration",
    ] {
        assert!(names.contains(&wanted), "{names:?}");
    }
    let (code, text, _) = run_args(&home, &["doctor"]);
    assert_eq!(code, 0);
    assert!(text.trim_end().ends_with("healthy"), "{text}");
}

// covers: C065
#[test]
fn doctor_fails_on_a_broken_configuration() {
    let home = temp_home("doctor-bad");
    json_at(&home, &["init"], 0);
    let file = home.join("config").join("config.toml");
    std::fs::write(&file, "schema_version = 2\nbogus = 1\n").expect("a bad file");
    let (code, stdout, stderr) = run_args(&home, &["--json", "doctor"]);
    assert_eq!(code, 7, "{stdout}{stderr}");
}
