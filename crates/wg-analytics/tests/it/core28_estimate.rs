#![allow(clippy::expect_used)]
//! CORE-28: the `estimate` and `retrospective` decisions of `wg-analytics`, over source rows built
//! in place. The expected values follow `analytics/estimation.py` and `analytics/retrospective.py`.

use wg_analytics::build;
use wg_analytics::core::estimate::{Request as EstimateRequest, estimate};
use wg_analytics::core::retrospective::{Request as RetroRequest, retrospective};
use wg_analytics::source::{AnalyticsInput, AttemptRow, TaskRow, VerificationRun};
use wg_types::Timestamp;

const WS: &str = "ws-1";

fn at(text: &str) -> Timestamp {
    Timestamp::parse_iso(text).expect("a valid ISO timestamp")
}

/// A task that is not accepted yet: the target of an estimate.
fn open_task(id: &str, category: &str) -> TaskRow {
    TaskRow {
        id: Some(id.to_owned()),
        workspace_id: Some(WS.to_owned()),
        key: Some(format!("WG-{id}")),
        title: Some(format!("task {id}")),
        status: Some("running".to_owned()),
        category: Some(category.to_owned()),
        ..TaskRow::default()
    }
}

/// An accepted task, comparable with the target when it shares the category.
fn accepted(id: &str, category: &str) -> TaskRow {
    TaskRow {
        status: Some("accepted".to_owned()),
        accepted_at: Some(at("2026-10-02T00:00:00Z")),
        ..open_task(id, category)
    }
}

fn attempt(
    id: &str,
    task: &str,
    status: &str,
    provider: &str,
    model: &str,
    duration_ms: i64,
) -> AttemptRow {
    AttemptRow {
        id: Some(id.to_owned()),
        workspace_id: Some(WS.to_owned()),
        task_id: Some(task.to_owned()),
        status: Some(status.to_owned()),
        provider: Some(provider.to_owned()),
        model: Some(model.to_owned()),
        duration_ms: Some(duration_ms),
        ..AttemptRow::default()
    }
}

fn run(task: &str, status: &str) -> VerificationRun {
    VerificationRun {
        id: Some(format!("run-{task}")),
        workspace_id: Some(WS.to_owned()),
        task_id: Some(task.to_owned()),
        status: Some(status.to_owned()),
        ..VerificationRun::default()
    }
}

fn source(
    tasks: Vec<TaskRow>,
    attempts: Vec<AttemptRow>,
    runs: Vec<VerificationRun>,
) -> AnalyticsInput {
    AnalyticsInput {
        tasks: tasks.into_iter().collect(),
        attempts: attempts.into_iter().collect(),
        verification_runs: runs.into_iter().collect(),
        ..AnalyticsInput::default()
    }
}

fn estimate_of(input: &AnalyticsInput, text_scores: &[(String, f64)]) -> serde_json::Value {
    let facts = build(input);
    let request = EstimateRequest {
        target: Some("t0"),
        category: Some("feature"),
        repositories: &[],
        method: "lexical",
        text_scores,
    };
    estimate(input, &facts, WS, &request)
}

#[test]
fn fewer_than_three_comparable_tasks_is_insufficient_data() {
    let input = source(
        vec![
            open_task("t0", "feature"),
            accepted("a", "feature"),
            accepted("b", "feature"),
        ],
        vec![],
        vec![],
    );
    let result = estimate_of(&input, &[]);
    assert_eq!(result["status"], "insufficient_data");
    assert_eq!(result["comparable_tasks"].as_array().map(Vec::len), Some(2));
    assert_eq!(result["min_sample"], 3);
    assert_eq!(
        result["message"],
        "only 2 comparable accepted task(s) found; at least 3 are required"
    );
}

#[test]
fn comparable_tasks_are_scored_and_ranked_by_category_and_text() {
    let input = source(
        vec![
            open_task("t0", "feature"),
            accepted("a", "feature"),
            accepted("b", "feature"),
            accepted("c", "feature"),
            accepted("d", "feature"),
            accepted("e", "bugfix"),
        ],
        vec![],
        vec![],
    );
    // a: category 0.35 + 0.4 * 1.0; b: 0.35 + 0.4 * 0.5; d: 0.35 + 0.4 * 0.25; c: category only.
    let scores = [
        ("a".to_owned(), 1.0),
        ("b".to_owned(), 0.5),
        ("d".to_owned(), 0.25),
    ];
    let result = estimate_of(&input, &scores);
    assert_eq!(result["status"], "ok");
    assert_eq!(result["sample_size"], 4);
    assert_eq!(result["method"], "lexical");
    let basis = result["basis"].as_array().expect("basis");
    let order: Vec<&str> = basis
        .iter()
        .map(|row| row["task_id"].as_str().expect("task id"))
        .collect();
    assert_eq!(order, ["a", "b", "d", "c"]);
    assert_eq!(basis[0]["score"], 0.75);
    assert_eq!(basis[1]["score"], 0.55);
    assert_eq!(basis[3]["score"], 0.35);
    assert_eq!(
        basis[0]["reasons"],
        serde_json::json!(["same category (feature)", "lexical text similarity 1.00"])
    );
    assert_eq!(
        basis[3]["reasons"],
        serde_json::json!(["same category (feature)"])
    );
}

#[test]
fn equal_scores_rank_by_task_id_descending() {
    let input = source(
        vec![
            open_task("t0", "feature"),
            accepted("a", "feature"),
            accepted("b", "feature"),
            accepted("c", "feature"),
        ],
        vec![],
        vec![],
    );
    let result = estimate_of(&input, &[]);
    let order: Vec<&str> = result["basis"]
        .as_array()
        .expect("basis")
        .iter()
        .map(|row| row["task_id"].as_str().expect("task id"))
        .collect();
    assert_eq!(order, ["c", "b", "a"]);
}

#[test]
fn agents_and_verification_are_summarised_per_population() {
    let input = source(
        vec![
            open_task("t0", "feature"),
            accepted("a", "feature"),
            accepted("b", "feature"),
            accepted("c", "feature"),
        ],
        vec![
            attempt("1", "a", "succeeded", "claude-code", "opus", 1000),
            attempt("2", "a", "failed", "claude-code", "opus", 3000),
            attempt("3", "b", "succeeded", "claude-code", "opus", 2000),
            attempt("4", "c", "succeeded", "codex", "gpt", 500),
        ],
        vec![run("a", "passed"), run("b", "failed"), run("c", "passed")],
    );
    let result = estimate_of(&input, &[]);
    let agents = result["agent_recommendations"].as_array().expect("agents");
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0]["provider"], "codex");
    assert_eq!(agents[0]["success_rate"], 1.0);
    assert_eq!(agents[0]["median_duration_ms"], 500.0);
    assert_eq!(agents[1]["provider"], "claude-code");
    assert_eq!(agents[1]["attempts"], 3);
    assert_eq!(agents[1]["succeeded"], 2);
    assert_eq!(agents[1]["success_rate"], 0.667);
    assert_eq!(agents[1]["median_duration_ms"], 2000.0);
    assert_eq!(result["verification"]["runs"], 3);
    assert_eq!(result["verification"]["passed"], 2);
    assert_eq!(result["verification"]["pass_rate"], 0.667);
}

#[test]
fn an_empty_window_writes_the_no_data_lines() {
    let input = AnalyticsInput::default();
    let facts = build(&input);
    let request = RetroRequest {
        window: (at("2026-10-01T00:00:00Z"), at("2026-10-15T00:00:00Z")),
        project: None,
        repository: None,
        project_id: None,
        repository_id: None,
        generated_at: "2026-10-15T00:00:00+00:00",
    };
    let retro = retrospective(&input, &facts, WS, &request, &|text: &str| text.to_owned())
        .expect("a retrospective over no rows");
    assert_eq!(retro.title, "Retrospective 2026-10-01 - 2026-10-15");
    assert!(
        retro
            .content
            .starts_with("# Retrospective: 2026-10-01 to 2026-10-15\n")
    );
    assert!(retro.content.contains("\n- no accepted tasks in period\n"));
    assert!(
        retro
            .content
            .contains("\n- no agent attempts finished in the period\n")
    );
    assert!(retro.content.contains("(n=0).\n"));
    assert!(retro.content.contains("\n- none recorded\n"));
    assert!(
        retro
            .content
            .ends_with("\n- No significant failure patterns detected.\n")
    );
    assert_eq!(retro.facts["tasks"]["accepted"], 0);
    assert_eq!(retro.highlights.len(), 3);
    assert!(retro.opportunities.is_empty());
}
