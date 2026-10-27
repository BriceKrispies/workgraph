//! The `experiment create, freeze, close, next, report, fit, predict` commands, run through the
//! real binary against a real SQLite database. The shapes, messages and exit codes follow the
//! Python CLI (`src/workgraph/cli/commands/experiments.py`).

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-experiment-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs the binary with `--home` and returns the exit code, stdout and stderr.
fn run(home: &Path, args: &[&str]) -> (i32, String, String) {
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

/// Runs a command with `--json` and parses its standard output.
fn json_of(home: &Path, args: &[&str]) -> (i32, Value) {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let (code, stdout, _) = run(home, &full);
    let value = serde_json::from_str(stdout.trim()).expect("the output is JSON");
    (code, value)
}

fn initialised(name: &str) -> PathBuf {
    let home = temp_home(name);
    let (code, _, stderr) = run(&home, &["init"]);
    assert_eq!(code, 0, "init failed: {stderr}");
    home
}

fn protocol() -> String {
    json!({
        "arms": [
            {"name": "small", "model": "claude-haiku-5-5"},
            {"name": "big", "model": "claude-sonnet-5-5"},
        ],
        "brief_template": "do the task",
        "primary_size": "src_lines",
        "bands": {"min": 1, "max": 5},
        "staircase": {"per_visit": 1, "up": 1, "down": 1, "max_reversals": 2, "max_trials": 4},
        "budget": {"trials": 10},
    })
    .to_string()
}

fn create_experiment(home: &Path, slug: &str) -> Value {
    let (code, created) = json_of(
        home,
        &[
            "experiment",
            "create",
            slug,
            "--title",
            "Haiku sizes",
            "--protocol",
            &protocol(),
        ],
    );
    assert_eq!(code, 0, "{created}");
    created
}

// covers: C069
#[test]
fn create_makes_a_draft_and_refuses_a_bad_slug_title_protocol_or_duplicate() {
    let home = initialised("create");
    let created = create_experiment(&home, "haiku-sizes");
    assert_eq!(created["status"], "draft");
    assert_eq!(created["slug"], "haiku-sizes");
    assert_eq!(created["frozen_at"], Value::Null);
    let (code, shown) = json_of(&home, &["experiment", "show", "haiku-sizes"]);
    assert_eq!(code, 0);
    assert_eq!(shown["experiment"]["id"], created["id"]);

    let (code, bad) = json_of(
        &home,
        &[
            "experiment",
            "create",
            "Bad Slug",
            "--title",
            "t",
            "--protocol",
            &protocol(),
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(
        bad["error"]["message"],
        "experiment slug 'Bad Slug' must be lowercase letters, digits and hyphens"
    );
    let (code, bad) = json_of(
        &home,
        &[
            "experiment",
            "create",
            "x",
            "--title",
            "  ",
            "--protocol",
            &protocol(),
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(
        bad["error"]["message"],
        "experiment title must not be empty"
    );
    let (code, bad) = json_of(
        &home,
        &[
            "experiment",
            "create",
            "x",
            "--title",
            "t",
            "--protocol",
            r#"{"arms":[{"name":"a","model":"haiku"}]}"#,
        ],
    );
    assert_eq!(code, 2);
    let message = bad["error"]["message"].as_str().expect("a message");
    assert!(message.starts_with("invalid experiment protocol: missing keys: bands, brief_template, budget, primary_size, staircase; "));
    assert!(message.contains("arms[0].model is the alias 'haiku'; pin a full model id"));
    assert_eq!(
        bad["error"]["details"]["problems"].as_array().map(Vec::len),
        Some(2)
    );
    let (code, _, stderr) = run(
        &home,
        &[
            "experiment",
            "create",
            "x",
            "--title",
            "t",
            "--protocol",
            "{nope",
        ],
    );
    assert_eq!(code, 2);
    assert!(stderr.contains("protocol is not valid JSON: Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"));
    let (code, dup) = json_of(
        &home,
        &[
            "experiment",
            "create",
            "haiku-sizes",
            "--title",
            "t",
            "--protocol",
            &protocol(),
        ],
    );
    assert_eq!(code, 3);
    assert_eq!(
        dup["error"]["message"],
        "experiment 'haiku-sizes' already exists in this workspace"
    );
}

// covers: C068, C071
#[test]
fn freeze_starts_the_experiment_and_close_stops_it() {
    let home = initialised("lifecycle");
    create_experiment(&home, "run-one");
    let (code, closed_early) = json_of(&home, &["experiment", "close", "run-one"]);
    assert_eq!(code, 3);
    assert_eq!(
        closed_early["error"]["message"],
        "run-one is draft; only a running experiment can be closed"
    );
    let (code, frozen) = json_of(&home, &["experiment", "freeze", "run-one"]);
    assert_eq!(code, 0);
    assert_eq!(frozen["status"], "running");
    assert!(frozen["frozen_at"].is_string());
    let (code, again) = json_of(&home, &["experiment", "freeze", "run-one"]);
    assert_eq!(code, 3);
    assert_eq!(
        again["error"]["message"],
        "run-one is already running; only a draft can be frozen"
    );
    let (code, closed) = json_of(&home, &["experiment", "close", "run-one"]);
    assert_eq!(code, 0);
    assert_eq!(closed["status"], "closed");
    let (code, _) = json_of(&home, &["experiment", "freeze", "missing"]);
    assert_eq!(code, 4);

    let (code, text, _) = run(&home, &["experiment", "list"]);
    assert_eq!(code, 0);
    assert!(text.contains("run-one") && text.contains("closed"));
    let (_, stdout, _) = run(
        &home,
        &[
            "experiment",
            "create",
            "second",
            "--title",
            "T",
            "--protocol",
            &protocol(),
        ],
    );
    assert!(stdout.starts_with("created experiment second\n  slug: second\n  status: draft\n"));
}

// covers: C073, C077
#[test]
fn next_plans_the_staircase_from_the_pool_and_report_summarises_it() {
    let home = initialised("next");
    let (code, task) = json_of(
        &home,
        &["task", "create", "Pool task", "--tag", "experiment-pool"],
    );
    assert_eq!(code, 0);
    let key = task["key"].as_str().expect("a key").to_owned();
    let metadata = json!({"replay": {"validated": true, "features": {"band": 4, "src_lines": 40}}});
    let (code, _, stderr) = run(
        &home,
        &["task", "update", &key, "--metadata", &metadata.to_string()],
    );
    assert_eq!(code, 0, "{stderr}");
    create_experiment(&home, "stair");

    let (code, draft) = json_of(&home, &["experiment", "next", "stair"]);
    assert_eq!(code, 3, "{draft}");
    assert_eq!(
        draft["error"]["message"],
        "experiment stair is draft; trials change only while it is running"
    );
    let (code, _) = json_of(&home, &["experiment", "freeze", "stair"]);
    assert_eq!(code, 0);

    let (code, planned) = json_of(&home, &["experiment", "next", "stair"]);
    assert_eq!(code, 0, "{planned}");
    assert_eq!(planned["done"], false);
    assert_eq!(planned["reason"], "planned");
    assert_eq!(planned["phase"], "staircase");
    assert_eq!(planned["trial"]["task_key"], key);
    assert_eq!(planned["trial"]["band"], 4);
    assert_eq!(planned["trial"]["seq"], 1);
    assert_eq!(planned["trial"]["requested_model"], "claude-haiku-5-5");
    assert_eq!(
        planned["trial"]["features"]["predicted"]["band_requested"],
        5
    );
    assert_eq!(planned["step"]["band"], 5);
    assert_eq!(planned["step"]["phase"], "staircase");

    let (code, pending) = json_of(&home, &["experiment", "next", "stair"]);
    assert_eq!(code, 0);
    assert_eq!(pending["reason"], "pending");
    assert_eq!(pending["trial"]["id"], planned["trial"]["id"]);
    let (_, text, _) = run(&home, &["experiment", "next", "stair"]);
    assert!(text.starts_with(&format!("pending: {key} (staircase) is not graded yet\n")));

    let (code, report) = json_of(&home, &["experiment", "report", "stair"]);
    assert_eq!(code, 0);
    assert_eq!(report["experiment"]["slug"], "stair");
    assert_eq!(report["arms"][0]["arm"], "small");
    assert_eq!(report["arms"][0]["trials"]["total"], 1);
    assert_eq!(
        report["arms"][0]["fit_reason"],
        "needs at least 4 graded trials with src_lines, has 0"
    );
    assert_eq!(report["arms"][0]["usage"]["pass"]["n"], 0);
    assert_eq!(report["arms"][1]["trials"]["total"], 0);
    let (code, text, _) = run(&home, &["experiment", "report", "stair"]);
    assert_eq!(code, 0);
    assert!(text.starts_with("experiment stair (running): phase staircase, band 5\n"));
    assert!(text.contains("fit: needs at least 4 graded trials with src_lines, has 0"));
    assert!(text.contains("failure causes: none"));

    let (code, shown) = json_of(&home, &["experiment", "show", "stair"]);
    assert_eq!(code, 0);
    assert_eq!(shown["trials"].as_array().map(Vec::len), Some(1));
}

// covers: C070, C074
#[test]
fn fit_and_predict_refuse_what_they_cannot_do() {
    let home = initialised("predict");
    let (code, fit) = json_of(&home, &["experiment", "fit", "--model", "claude-haiku-5-5"]);
    assert_eq!(code, 2);
    assert_eq!(
        fit["error"]["message"],
        "the predictor for claude-haiku-5-5 needs at least 4 graded trials with src_lines, \
         src_files and packages; has 0"
    );
    let (code, fit) = json_of(
        &home,
        &["experiment", "fit", "--model", "m", "--repo-penalty", "-1"],
    );
    assert_eq!(code, 2);
    assert_eq!(fit["error"]["message"], "penalties must be >= 0");
    let (code, fit) = json_of(
        &home,
        &["experiment", "fit", "--model", "m", "--samples", "-3"],
    );
    assert_eq!(code, 2);
    assert_eq!(fit["error"]["message"], "samples must be >= 0, got -3");

    let (code, predict) = json_of(&home, &["experiment", "predict", "--model", "m"]);
    assert_eq!(code, 2);
    assert_eq!(
        predict["error"]["message"],
        "predict needs --task or all of --repo, --lines, --files, --packages; \
         missing --repo, --lines, --files, --packages"
    );
    let (code, predict) = json_of(
        &home,
        &[
            "experiment",
            "predict",
            "--model",
            "m",
            "--task",
            "WG-1",
            "--lines",
            "3",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(
        predict["error"]["message"],
        "give either --task or --repo, --lines, --files and --packages, not both"
    );
    let (code, predict) = json_of(
        &home,
        &[
            "experiment",
            "predict",
            "--model",
            "m",
            "--repo",
            "r",
            "--lines",
            "40",
            "--files",
            "2",
            "--packages",
            "",
        ],
    );
    assert_eq!(code, 4);
    assert_eq!(
        predict["error"]["message"],
        "no predictor fit for model m; create one with `workgraph experiment fit --model m`"
    );
    let (code, predict) = json_of(
        &home,
        &[
            "experiment",
            "predict",
            "--model",
            "m",
            "--repo",
            "r",
            "--lines",
            "-1",
            "--files",
            "2",
            "--packages",
            "a",
        ],
    );
    assert_eq!(code, 2);
    assert_eq!(
        predict["error"]["message"],
        "src_lines and src_files must be >= 0"
    );
    let (code, _, stderr) = run(
        &home,
        &[
            "experiment",
            "predict",
            "--model",
            "m",
            "--repo",
            "r",
            "--lines",
            "x",
            "--files",
            "2",
            "--packages",
            "a",
        ],
    );
    assert_eq!(code, 2);
    assert!(stderr.contains("Invalid value for '--lines': 'x' is not a valid float."));
}
