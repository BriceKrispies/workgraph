//! IF-14 writes: the `classify` writes (request, confirm, promote, reclassify, gold, evaluate) and
//! the `search` options `--reindex` and `--similar-to`, run through the real binary against a real
//! SQLite database. The model is the offline heuristic provider, selected per command, so no
//! network is used. The workspace is written through the store, as the IF-05 tests do.

#![allow(clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use wg_model::Write;
use wg_model::entity::Workspace;
use wg_runtime::App;
use wg_types::Timestamp;

fn temp_home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-if14-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A home with the workspace `acme` (prefix `AC`).
fn home_with_workspace(name: &str) -> PathBuf {
    let home = temp_home(name);
    {
        let mut app = App::open(Some(&home)).expect("the app opens");
        let at = Timestamp::parse_iso("2026-01-01T00:00:00Z").expect("a timestamp");
        let id = wg_model::derive_id(&[9; 32], 0);
        app.runtime
            .store
            .commit_decision(&wg_model::Decision::new(()).write(Write::insert(Workspace {
                id,
                slug: "acme".to_owned(),
                name: "Acme".to_owned(),
                description: String::new(),
                key_prefix: "AC".to_owned(),
                next_task_number: 1,
                settings: serde_json::Map::new(),
                archived_at: None,
                created_at: at,
                updated_at: at,
            })))
            .expect("the workspace is written");
    }
    home
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

fn create(home: &Path, title: &str, description: &str) -> Value {
    json(
        home,
        &["task", "create", title, "--description", description],
        0,
    )
}

/// Two tasks, AC-1 and AC-2, that the heuristic classifies differently.
fn home_with_tasks(name: &str) -> PathBuf {
    let home = home_with_workspace(name);
    let first = create(
        &home,
        "Fix login crash",
        "The login page crashes when the session token expires.",
    );
    assert_eq!(first["key"], "AC-1");
    let second = create(
        &home,
        "Add CSV export",
        "Users need a feature to export the report as a CSV file.",
    );
    assert_eq!(second["key"], "AC-2");
    home
}

const HEURISTIC: &[&str] = &["--provider", "heuristic", "--model", "heuristic-v1"];

// covers: C038, C039
#[test]
fn request_now_stores_a_succeeded_row_and_reuses_it() {
    let home = home_with_tasks("now");
    let mut args = vec!["classify", "request", "AC-1", "--now"];
    args.extend_from_slice(HEURISTIC);
    let first = json(&home, &args, 0);
    assert_eq!(first["status"], "succeeded");
    assert_eq!(first["source"], "model");
    assert_eq!(first["provider"], "heuristic");
    assert_eq!(first["model"], "heuristic-v1");
    assert_eq!(first["is_current"], true);
    assert!(first["category"].is_string(), "{first}");
    let again = json(&home, &args, 0);
    assert_eq!(
        again["id"], first["id"],
        "a succeeded row for the same input is reused"
    );
    let shown = json(&home, &["classify", "show", "AC-1"], 0);
    assert_eq!(shown["current"]["id"], first["id"]);
    assert_eq!(shown["history"].as_array().map(Vec::len), Some(1));
}

// covers: C040
#[test]
fn request_without_now_queues_a_classification_job() {
    let home = home_with_tasks("queued");
    let queued = json(&home, &["classify", "request", "AC-2"], 0);
    assert_eq!(queued["task_key"], "AC-2");
    assert!(queued["job_id"].is_string(), "{queued}");
    assert_eq!(queued["status"], "queued");
}

// covers: C041, C042
#[test]
fn confirm_is_authoritative_and_joins_the_gold_set() {
    let home = home_with_tasks("confirm");
    let mut args = vec!["classify", "request", "AC-1", "--now"];
    args.extend_from_slice(HEURISTIC);
    json(&home, &args, 0);
    let confirmed = json(
        &home,
        &[
            "classify",
            "confirm",
            "AC-1",
            "feature",
            "--note",
            "a human said so",
        ],
        0,
    );
    assert_eq!(confirmed["category"], "feature");
    assert_eq!(confirmed["source"], "manual");
    assert!(confirmed["confirmed_by"].is_string(), "{confirmed}");
    let shown = json(&home, &["classify", "show", "AC-1"], 0);
    assert_eq!(shown["current"]["source"], "manual");
    let gold = json(&home, &["classify", "gold"], 0);
    assert_eq!(gold.as_array().map(Vec::len), Some(1), "{gold}");
    assert_eq!(gold[0]["category"], "feature");
    // A model run cannot replace the human classification.
    let rerun = json(&home, &args, 0);
    assert_ne!(rerun["id"], confirmed["id"]);
    let after = json(&home, &["classify", "show", "AC-1"], 0);
    assert_eq!(after["current"]["id"], confirmed["id"]);
}

// covers: C043
#[test]
fn gold_with_a_category_adds_a_label_and_rejects_unknown_categories() {
    let home = home_with_tasks("gold");
    let added = json(
        &home,
        &[
            "classify",
            "gold",
            "--category",
            "bug_fix",
            "--task",
            "AC-2",
        ],
        0,
    );
    assert_eq!(added["category"], "bug_fix");
    assert!(added["id"].is_string(), "{added}");
    let gold = json(&home, &["classify", "gold"], 0);
    assert_eq!(gold.as_array().map(Vec::len), Some(1));
    let (code, _, _) = run(
        &home,
        &["classify", "gold", "--category", "nope", "--task", "AC-2"],
    );
    assert_ne!(code, 0);
}

// covers: C044
#[test]
fn evaluate_scores_the_gold_set_with_the_given_model() {
    let home = home_with_tasks("evaluate");
    json(
        &home,
        &[
            "classify",
            "gold",
            "--category",
            "bug_fix",
            "--task",
            "AC-1",
        ],
        0,
    );
    json(
        &home,
        &[
            "classify",
            "gold",
            "--category",
            "feature",
            "--task",
            "AC-2",
        ],
        0,
    );
    let mut args = vec!["classify", "evaluate"];
    args.extend_from_slice(HEURISTIC);
    let evaluation = json(&home, &args, 0);
    assert_eq!(evaluation["sample_size"], 2);
    assert_eq!(evaluation["failures"], 0);
    assert_eq!(evaluation["provider"], "heuristic");
    assert_eq!(evaluation["results"].as_array().map(Vec::len), Some(2));
    let history = json(&home, &["classify", "evaluations"], 0);
    assert_eq!(history.as_array().map(Vec::len), Some(1));
}

// covers: C045
#[test]
fn reclassify_queues_one_job_per_task_as_a_new_batch() {
    let home = home_with_tasks("reclassify");
    let batch = json(
        &home,
        &[
            "classify",
            "reclassify",
            "--provider",
            "heuristic",
            "--model",
            "heuristic-v1",
        ],
        0,
    );
    assert_eq!(batch["tasks"], 2);
    assert_eq!(batch["total_matching"], 2);
    assert_eq!(batch["job_ids"].as_array().map(Vec::len), Some(2));
    assert!(batch["batch_id"].is_string(), "{batch}");
}

// covers: C046
#[test]
fn promote_makes_a_succeeded_row_current() {
    let home = home_with_tasks("promote");
    let mut args = vec!["classify", "request", "AC-1", "--now"];
    args.extend_from_slice(HEURISTIC);
    let first = json(&home, &args, 0);
    json(&home, &["classify", "confirm", "AC-1", "research"], 0);
    let promoted = json(
        &home,
        &[
            "classify",
            "promote",
            first["id"].as_str().unwrap_or_default(),
        ],
        0,
    );
    assert_eq!(promoted["id"], first["id"]);
    assert_eq!(promoted["is_current"], true);
    let shown = json(&home, &["classify", "show", "AC-1"], 0);
    assert_eq!(shown["current"]["id"], first["id"]);
}

// covers: C133
#[test]
fn search_reindex_queues_an_embedding_per_task_and_similar_is_lexical_without_vectors() {
    let home = home_with_tasks("reindex");
    let reindexed = json(&home, &["search", "login", "--reindex"], 0);
    assert_eq!(reindexed["enqueued"], 2);
    let similar = json(&home, &["search", "login", "--similar-to", "AC-1"], 0);
    assert_eq!(similar["method"], "lexical");
    assert!(similar["similar"].is_array(), "{similar}");
    let hybrid = json(&home, &["search", "login", "--mode", "hybrid"], 0);
    assert_eq!(hybrid["semantic_available"], false);
    assert!(
        hybrid["note"]
            .as_str()
            .unwrap_or_default()
            .contains("no embeddings stored"),
        "{hybrid}"
    );
}
