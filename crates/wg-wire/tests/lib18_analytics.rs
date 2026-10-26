//! LIB-18: the DTOs of the analytics, reports, search and classification area. Each DTO lists its
//! fields in the order of its `FIELDS` constant, and the timestamps print as the API prints them.

#![allow(clippy::expect_used)]

use rpds::VectorSync;
use serde_json::{Map, json};
use wg_json::{LocalDateTime, Value, http_json};
use wg_model::entity::JsonObject;
use wg_model::entity::Real;
use wg_model::entity::classification::{Classification, ClassificationEvaluation};
use wg_types::{Id, Timestamp, parse_id};
use wg_wire::dto::agent_time_out;
use wg_wire::dto::analytics_compare_out;
use wg_wire::dto::analytics_filters::{self, Filters};
use wg_wire::dto::analytics_mcp_shapes;
use wg_wire::dto::analytics_overview;
use wg_wire::dto::analytics_query;
use wg_wire::dto::analytics_query_out;
use wg_wire::dto::analytics_report_entry;
use wg_wire::dto::classification_disagreement;
use wg_wire::dto::classification_evaluation;
use wg_wire::dto::classification_job;
use wg_wire::dto::classification_out;
use wg_wire::dto::classification_task;
use wg_wire::dto::coupling_out;
use wg_wire::dto::estimate_out;
use wg_wire::dto::failures_out;
use wg_wire::dto::metric_definition::{self, MetricDefinition};
use wg_wire::dto::retrospective_out;
use wg_wire::dto::search_result::{self, Hit};
use wg_wire::dto::subagents_out;
use wg_wire::dto::task_history_row::{self, TaskHistoryRow};
use wg_wire::dto::{
    analytics_compare_body, analytics_estimate_body, analytics_export_body, retro_body,
};

fn at(micros: u32) -> Timestamp {
    Timestamp::from_components(2026, 10, 25, 13, 56, 0, micros).expect("timestamp")
}

fn id_of(text: &str) -> Id {
    parse_id(text).expect("id")
}

fn names(value: &Value) -> Vec<String> {
    match value {
        Value::Model { fields, .. } => fields.iter().map(|(k, _)| k.clone()).collect(),
        Value::Dict(fields) => fields
            .iter()
            .map(|(k, _)| {
                k.code_points()
                    .iter()
                    .filter_map(|&c| char::from_u32(c))
                    .collect()
            })
            .collect(),
        other => panic!("not a model or dict: {other:?}"),
    }
}

fn printed(value: &Value) -> String {
    http_json(value).expect("dump")
}

fn sample_classification() -> Classification {
    Classification {
        id: id_of("00000000-0000-4000-8000-0000000000c1"),
        task_id: id_of("00000000-0000-4000-8000-0000000000b1"),
        source: "model".into(),
        status: "succeeded".into(),
        provider: Some("heuristic".into()),
        model: Some("heuristic-v1".into()),
        prompt_version: Some("classify-v1".into()),
        taxonomy_version: "2026.1".into(),
        input_hash: "abc".into(),
        category: Some("bug_fix".into()),
        subtype: Some("bug_fix:auth".into()),
        domain: Some("auth".into()),
        affected_systems: VectorSync::new_sync().push_back("api".to_string()),
        complexity: Some("small".into()),
        tags: VectorSync::new_sync().push_back("crash".to_string()),
        confidence: Some(Real(0.564)),
        rationale: Some("keyword score".into()),
        raw_output: None,
        validation_status: "valid".into(),
        error: None,
        duration_ms: Some(0),
        is_current: true,
        confirmed_by: None,
        batch_id: None,
        created_by: "system:classifier".into(),
        created_at: at(411_471),
    }
}

fn sample_evaluation() -> ClassificationEvaluation {
    let mut per_class: JsonObject = Map::new();
    per_class.insert("bug_fix".into(), json!({"precision": 0.5}));
    ClassificationEvaluation {
        id: id_of("00000000-0000-4000-8000-0000000000e1"),
        workspace_id: id_of("00000000-0000-4000-8000-000000000001"),
        provider: "heuristic".into(),
        model: "heuristic-v1".into(),
        prompt_version: "classify-v1".into(),
        taxonomy_version: "2026.1".into(),
        sample_size: 10,
        failures: 1,
        accuracy: Some(Real(0.9)),
        macro_f1: None,
        per_class,
        confusion: Map::new(),
        results: VectorSync::new_sync(),
        created_at: at(0),
    }
}

#[test]
fn classification_out_lists_its_fields_and_prints_z_timestamps() {
    let value = classification_out::from_model(&sample_classification());
    assert_eq!(names(&value), classification_out::FIELDS);
    let text = printed(&value);
    assert!(
        text.contains("\"created_at\":\"2026-10-25T13:56:00.411471Z\""),
        "{text}"
    );
    assert!(text.contains("\"tags\""), "{text}");
}

#[test]
fn task_classifications_wraps_current_and_history() {
    let current = classification_out::from_model(&sample_classification());
    let history = vec![current.clone()];
    let value = classification_task::from_parts(
        "00000000-0000-4000-8000-0000000000a1",
        "DEMO-1",
        Some(current),
        history,
        false,
    );
    assert_eq!(names(&value), classification_task::FIELDS);
    assert!(printed(&value).contains("\"note\":\"Model classifications are inferences"));
}

#[test]
fn classification_job_and_evaluation_are_dicts_with_isoformat_times() {
    let job = classification_job::from_parts("j", "t", "DEMO-1", "queued");
    assert_eq!(names(&job), classification_job::FIELDS);

    let evaluation = classification_evaluation::from_model(&sample_evaluation());
    assert_eq!(names(&evaluation), classification_evaluation::FIELDS);
    let text = printed(&evaluation);
    assert!(
        text.contains("\"created_at\":\"2026-10-25T13:56:00+00:00\""),
        "{text}"
    );
    assert!(text.contains("\"bug_fix\""), "{text}");
}

#[test]
fn disagreement_items_list_their_fields() {
    let item = classification_disagreement::item_from_model(&sample_classification());
    assert_eq!(names(&item), classification_disagreement::ITEM_FIELDS);
    let entry = classification_disagreement::from_parts("t", "DEMO-1", vec![item]);
    assert_eq!(names(&entry), classification_disagreement::FIELDS);
}

#[test]
fn search_result_and_hit_print_as_models() {
    let hit = search_result::hit_from_parts(&Hit {
        task_id: "t",
        key: "DEMO-4",
        title: "Improve token validation",
        status: "accepted",
        category: Some("performance"),
        score: 1.0292,
        lexical_rank: Some(1),
        semantic_rank: None,
        similarity: None,
        snippet: Some("Improve [token] validation"),
    });
    assert_eq!(names(&hit), search_result::HIT_FIELDS);
    let result = search_result::from_parts("token", "lexical", false, None, vec![hit], None);
    assert_eq!(names(&result), search_result::FIELDS);
    assert!(printed(&result).contains("\"semantic_available\":false"));
}

#[test]
fn filters_echo_prints_datetimes_in_the_z_form() {
    let filters = Filters {
        since: Some(at(0)),
        ..Filters::default()
    };
    let value = analytics_filters::echo(&filters);
    assert_eq!(names(&value), analytics_filters::FIELDS);
    assert!(
        printed(&value).contains("\"since\":\"2026-10-25T13:56:00Z\""),
        "{}",
        printed(&value)
    );
}

#[test]
fn query_echo_and_result_list_their_fields() {
    let filters = analytics_filters::echo(&Filters::default());
    let echo = analytics_query::echo("tasks_created", &["period"], "week", filters, 500);
    assert_eq!(names(&echo), analytics_query::FIELDS);

    let provenance = analytics_query_out::provenance("SELECT 1", "snapshot", at(0));
    assert_eq!(names(&provenance), analytics_query_out::PROVENANCE_FIELDS);
    assert!(printed(&provenance).contains("\"generated_at\":\"2026-10-25T13:56:00+00:00\""));

    let metric = metric_definition::from_parts(&MetricDefinition {
        id: "tasks_created",
        family: "throughput",
        title: "Tasks created",
        definition: "Number of tasks created in the period.",
        unit: "tasks",
        statistic: "count",
        time_column: "created_at",
        dimensions: vec!["agent", "category", "period"],
        caveats: vec![],
    });
    assert_eq!(names(&metric), metric_definition::FIELDS);

    let result = analytics_query_out::from_parts(
        metric,
        echo,
        vec![],
        0,
        provenance,
        &["Metrics describe observed associations."],
    );
    assert_eq!(names(&result), analytics_query_out::FIELDS);
}

#[test]
fn compare_rows_put_group_keys_first_then_the_values() {
    let row = analytics_compare_out::row(
        vec![("category".to_string(), Value::Null)],
        [
            Value::Int(1),
            Value::Int(2),
            Value::Int(1),
            Value::Null,
            Value::Int(3),
            Value::Int(4),
        ],
    );
    assert_eq!(
        names(&row),
        [
            "category",
            "a",
            "b",
            "delta",
            "relative_change",
            "n_a",
            "n_b"
        ]
    );
    let value = analytics_compare_out::from_parts(
        Value::Null,
        "value",
        (at(0), at(0)),
        (at(0), at(0)),
        vec![row],
        &[],
    );
    assert_eq!(names(&value), analytics_compare_out::FIELDS);
    assert!(printed(&value).contains("\"period_a\""));
}

#[test]
fn overview_has_eighteen_sections_in_order() {
    let sections: [Value; 18] = std::array::from_fn(|_| Value::List(Vec::new()));
    let value = analytics_overview::from_parts(sections);
    assert_eq!(names(&value), analytics_overview::FIELDS);
    assert_eq!(analytics_overview::FIELDS.len(), 18);
}

#[test]
fn task_history_row_prints_accepted_at_without_offset() {
    let row = task_history_row::from_parts(&TaskHistoryRow {
        key: "DEMO-1",
        title: "Fix crash",
        category: None,
        status: "accepted",
        attempts: Some(1),
        retries: Some(0),
        execution_ms: None,
        queue_wait_ms: None,
        elapsed_ms: Some(1000),
        commits: Some(2),
        churn_insertions: Some(10),
        churn_deletions: Some(3),
        final_insertions: Some(8),
        final_deletions: Some(1),
        files_touched: Some(2),
        repositories_touched: Some(1),
        reopen_count: Some(0),
        accepted_at: LocalDateTime {
            year: 2026,
            month: 10,
            day: 25,
            hour: 13,
            minute: 56,
            second: 0,
            microsecond: 0,
            offset_seconds: None,
        },
    });
    assert_eq!(names(&row), task_history_row::FIELDS);
    assert!(printed(&row).contains("\"accepted_at\":\"2026-10-25T13:56:00\""));
}

#[test]
fn report_entries_retrospectives_and_failures_list_their_fields() {
    let report =
        analytics_report_entry::from_parts("r", "retrospective", "Retro", "model", at(0), "body");
    assert_eq!(names(&report), analytics_report_entry::FIELDS);

    let retro = retrospective_out::from_parts("r", "Retro", "model", "body", Value::Null);
    assert_eq!(names(&retro), retrospective_out::FIELDS);

    let finding = failures_out::finding(
        "dependency_bottleneck",
        "Dependency bottleneck",
        "high",
        Some(0.5),
        3,
        Value::Null,
        "fp",
    );
    assert_eq!(names(&finding), failures_out::FINDING_FIELDS);
    let report = failures_out::from_parts(at(0), vec![finding], "note");
    assert_eq!(names(&report), failures_out::FIELDS);
}

#[test]
fn coupling_report_and_summary_list_their_fields() {
    let summary = coupling_out::summary(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&summary), coupling_out::SUMMARY_FIELDS);
    let report = coupling_out::from_parts(std::array::from_fn(|_| Value::List(Vec::new())));
    assert_eq!(names(&report), coupling_out::FIELDS);
}

#[test]
fn estimate_variants_list_their_fields() {
    let refusal = estimate_out::insufficient("too few", Vec::new(), 5);
    assert_eq!(names(&refusal), estimate_out::INSUFFICIENT_FIELDS);

    let basis = estimate_out::basis_row("id", "DEMO-1", "Title", 0.5, &["same category"]);
    assert_eq!(names(&basis), estimate_out::BASIS_FIELDS);
    let distribution = estimate_out::distribution(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&distribution), estimate_out::DISTRIBUTION_FIELDS);
    let verification = estimate_out::verification(3, 2, Some(0.667));
    assert_eq!(names(&verification), estimate_out::VERIFICATION_FIELDS);

    let ok = estimate_out::ok(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&ok), estimate_out::OK_FIELDS);
}

#[test]
fn agent_time_and_bottleneck_reports_list_their_fields() {
    let window = agent_time_out::window(None, Some(at(0)));
    assert_eq!(names(&window), agent_time_out::WINDOW_FIELDS);
    assert!(printed(&window).contains("\"until\":\"2026-10-25T13:56:00+00:00\""));

    let totals = agent_time_out::totals(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&totals), agent_time_out::TOTALS_FIELDS);
    let class = agent_time_out::class_row(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&class), agent_time_out::CLASS_FIELDS);
    let group = agent_time_out::group_row(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&group), agent_time_out::GROUP_FIELDS);
    let slowest = agent_time_out::slowest_call(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&slowest), agent_time_out::SLOWEST_FIELDS);
    let concurrency = agent_time_out::concurrency(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&concurrency), agent_time_out::CONCURRENCY_FIELDS);

    let breakdown =
        agent_time_out::breakdown("class", window.clone(), None, totals.clone(), &[], vec![]);
    assert_eq!(names(&breakdown), agent_time_out::BREAKDOWN_FIELDS);

    let bottlenecks =
        agent_time_out::bottlenecks(window, totals, vec![], vec![], concurrency, vec![], &[]);
    assert_eq!(names(&bottlenecks), agent_time_out::BOTTLENECKS_FIELDS);
}

#[test]
fn subagent_report_rows_list_their_fields() {
    let summary: [Value; 13] = std::array::from_fn(|_| Value::Null);
    let period = subagents_out::period_row("2026-10-26", "claude", summary.clone());
    assert_eq!(names(&period)[..2], ["period", "model"]);
    assert_eq!(names(&period)[2..], subagents_out::SUMMARY_FIELDS);

    let total = subagents_out::model_total("claude", summary.clone());
    assert_eq!(names(&total)[1..], subagents_out::SUMMARY_FIELDS);

    let size = subagents_out::size_row("claude", None, std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&size), subagents_out::BY_SIZE_FIELDS);

    let report = subagents_out::from_parts(std::array::from_fn(|_| Value::Null));
    assert_eq!(names(&report), subagents_out::FIELDS);
}

#[test]
fn mcp_wrappers_hold_their_list_under_one_key() {
    assert_eq!(names(&analytics_mcp_shapes::catalog(vec![])), ["metrics"]);
    assert_eq!(names(&analytics_mcp_shapes::history(vec![])), ["tasks"]);
}

#[test]
fn input_bodies_list_their_fields() {
    assert_eq!(analytics_compare_body::FIELDS[0], "metric");
    assert_eq!(analytics_estimate_body::FIELDS[0], "task");
    assert_eq!(analytics_export_body::FIELDS[0], "format");
    assert_eq!(retro_body::FIELDS[0], "period_days");
}
