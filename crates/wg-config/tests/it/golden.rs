//! Golden vectors for `wg_config::paths` (GV-18).
//!
//! `golden/paths/vectors.json` is produced by `tools/golden/gen_paths.py` from the Python
//! `AppPaths`. Each case fixes an operating system and an environment, and records every resolved
//! location. Every case must match.

use std::error::Error;
use std::path::PathBuf;

use serde_json::Value;
use wg_config::paths::{AppPaths, Environment, Os};

type TestResult = Result<(), Box<dyn Error>>;

/// The attributes the golden file records, with the accessor that returns each.
const ATTRIBUTES: [&str; 13] = [
    "config_dir",
    "data_dir",
    "log_dir",
    "config_file",
    "workspace_config_dir",
    "database_file",
    "analytics_file",
    "artifacts_dir",
    "worktrees_dir",
    "spool_dir",
    "backups_dir",
    "runs_dir",
    "api_token_file",
];

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../golden/paths/vectors.json")
}

fn variable(env: &Value, name: &str) -> Option<String> {
    env.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn environment(case: &Value) -> Result<Environment, Box<dyn Error>> {
    let os = match case["os"].as_str().ok_or("case has no os")? {
        "windows" => Os::Windows,
        "linux" => Os::Linux,
        "macos" => Os::MacOs,
        other => return Err(format!("unknown os {other}").into()),
    };
    let env = &case["env"];
    let home = match os {
        Os::Windows => variable(env, "USERPROFILE"),
        Os::Linux | Os::MacOs => variable(env, "HOME"),
    };
    Ok(Environment {
        os,
        workgraph_home: variable(env, "WORKGRAPH_HOME"),
        home,
        local_appdata: variable(env, "LOCALAPPDATA"),
        xdg_config_home: variable(env, "XDG_CONFIG_HOME"),
        xdg_data_home: variable(env, "XDG_DATA_HOME"),
        xdg_state_home: variable(env, "XDG_STATE_HOME"),
    })
}

fn actual(paths: &AppPaths, attribute: &str) -> String {
    let path = match attribute {
        "config_dir" => paths.config_dir(),
        "data_dir" => paths.data_dir(),
        "log_dir" => paths.log_dir(),
        "config_file" => paths.config_file(),
        "workspace_config_dir" => paths.workspace_config_dir(),
        "database_file" => paths.database_file(),
        "analytics_file" => paths.analytics_file(),
        "artifacts_dir" => paths.artifacts_dir(),
        "worktrees_dir" => paths.worktrees_dir(),
        "spool_dir" => paths.spool_dir(),
        "backups_dir" => paths.backups_dir(),
        "runs_dir" => paths.runs_dir(),
        "api_token_file" => paths.api_token_file(),
        other => return format!("<no accessor for {other}>"),
    };
    path.as_str().to_owned()
}

#[test]
fn paths_vectors_pass() -> TestResult {
    let text = std::fs::read_to_string(golden_path())?;
    let document: Value = serde_json::from_str(&text)?;
    let cases = document["cases"]
        .as_array()
        .ok_or("cases is not an array")?;
    assert!(!cases.is_empty(), "the golden file has no cases");

    let mut mismatches = Vec::new();
    for case in cases {
        let id = case["id"].as_str().ok_or("case has no id")?;
        let env = environment(case)?;
        let paths = AppPaths::resolve(&env).map_err(|error| format!("{id}: {error}"))?;
        for attribute in ATTRIBUTES {
            let expected = case["expected"][attribute]
                .as_str()
                .ok_or_else(|| format!("{id}: no expected {attribute}"))?;
            let got = actual(&paths, attribute);
            if got != expected {
                mismatches.push(format!(
                    "{id} {attribute}: expected {expected:?}, got {got:?}"
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} cases differ:\n{}",
        mismatches.len(),
        cases.len(),
        mismatches.join("\n")
    );
    Ok(())
}
