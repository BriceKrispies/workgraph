//! `analytics metrics` and `analytics query` (IF-15, C004/C005): the source loader, the request
//! checks, the result document and the human view, without a store.

use std::error::Error;

use wg_analytics::source::AnalyticsInput;
use wg_analytics_feature::spec::args;
use wg_analytics_feature::spec::dto::AnalyticsSource;
use wg_analytics_feature::spec::load;
use wg_analytics_feature::spec::query::{self, QueryRequest};
use wg_analytics_feature::spec::view;
use wg_json::{PyStr, Value};
use wg_render::Console;
use wg_rows::Raw;
use wg_types::{ErrorCode, Timestamp};

type TestResult = Result<(), Box<dyn Error>>;

fn source(table: &str, columns: &[&str], rows: Vec<Vec<Raw>>) -> AnalyticsSource {
    AnalyticsSource {
        table: table.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        rows,
    }
}

fn text(value: &str) -> Raw {
    Raw::Text(value.to_owned())
}

fn key(value: &Value, name: &str) -> Option<Value> {
    match value {
        Value::Dict(entries) => entries
            .iter()
            .find(|(k, _)| {
                k.code_points()
                    .iter()
                    .copied()
                    .eq(name.chars().map(u32::from))
            })
            .map(|(_, v)| v.clone()),
        _ => None,
    }
}

fn keys(value: &Value) -> Vec<String> {
    match value {
        Value::Dict(entries) => entries
            .iter()
            .map(|(k, _)| {
                k.code_points()
                    .iter()
                    .filter_map(|c| char::from_u32(*c))
                    .collect::<String>()
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[test]
fn loader_reads_timestamps_booleans_and_missing_columns() -> TestResult {
    let tasks = source(
        "tasks",
        &["id", "workspace_id", "status", "created_at", "auto_execute"],
        vec![
            vec![
                text("t1"),
                text("w1"),
                text("accepted"),
                text("2026-10-01T12:00:00.000000+00:00"),
                Raw::Int(1),
            ],
            vec![
                text("t2"),
                text("w1"),
                Raw::Null,
                text("not a time"),
                Raw::Int(0),
            ],
        ],
    );
    let input: AnalyticsInput = load::analytics_input(&[tasks]);
    let rows: Vec<_> = input.tasks.iter().cloned().collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id.as_deref(), Some("t1"));
    assert_eq!(rows[0].status.as_deref(), Some("accepted"));
    assert_eq!(
        rows[0].created_at,
        Some(Timestamp::parse_iso("2026-10-01T12:00:00Z")?)
    );
    assert_eq!(rows[0].auto_execute, Some(true));
    assert_eq!(rows[1].status, None);
    assert_eq!(rows[1].created_at, None, "an unreadable timestamp is NULL");
    assert_eq!(rows[1].auto_execute, Some(false));
    assert_eq!(
        rows[0].title, None,
        "a column the copy does not have is NULL"
    );
    assert!(input.commits.is_empty(), "an absent table has no rows");
    Ok(())
}

#[test]
fn loader_copies_dependency_and_relation_rows() -> TestResult {
    let sources = [
        source(
            "task_dependencies",
            &["task_id", "depends_on_id"],
            vec![vec![text("a"), text("b")]],
        ),
        source(
            "repository_dependencies",
            &["from_repository_id", "to_repository_id"],
            vec![vec![text("r1"), text("r2")]],
        ),
        source(
            "task_relations",
            &["source_task_id", "target_task_id", "relation_type"],
            vec![vec![text("a"), text("c"), text("blocks")]],
        ),
        source(
            "task_acceptances",
            &["task_id", "decision"],
            vec![vec![text("a"), text("accepted")]],
        ),
    ];
    let input = load::analytics_input(&sources);
    let dependency = input
        .task_dependencies
        .first()
        .ok_or("no task dependency")?;
    assert_eq!(dependency.depends_on_id.as_deref(), Some("b"));
    let repo = input
        .repository_dependencies
        .first()
        .ok_or("no repo dependency")?;
    assert_eq!(repo.to_repository_id.as_deref(), Some("r2"));
    let relation = input.relations.first().ok_or("no relation")?;
    assert_eq!(relation.target_task_id.as_deref(), Some("c"));
    let acceptance = input.acceptances.first().ok_or("no acceptance")?;
    assert_eq!(acceptance.decision.as_deref(), Some("accepted"));
    assert!(input.subagent_runs.is_empty());
    Ok(())
}

#[test]
fn definitions_list_every_metric_with_the_python_keys() -> TestResult {
    let defs = query::definitions();
    let Value::List(items) = &defs else {
        return Err("definitions is not a list".into());
    };
    assert!(
        items.len() >= 37,
        "the registry has 37 metrics, got {}",
        items.len()
    );
    assert_eq!(
        keys(&items[0]),
        [
            "id",
            "family",
            "title",
            "definition",
            "unit",
            "statistic",
            "time_column",
            "dimensions",
            "caveats",
        ]
    );
    Ok(())
}

#[test]
fn unknown_metric_and_period_are_validation_failures() {
    let unknown = QueryRequest {
        metric: "drop_table".to_owned(),
        period: "week".to_owned(),
        ..QueryRequest::default()
    };
    let err = query::check_request(&unknown).err();
    assert_eq!(err.map(|e| e.code), Some(ErrorCode::ValidationFailed));

    let known = QueryRequest {
        metric: "tasks_created".to_owned(),
        period: "fortnight".to_owned(),
        ..QueryRequest::default()
    };
    assert!(query::check_request(&known).is_err());
}

#[test]
fn query_document_keeps_the_python_field_order() -> TestResult {
    let request = QueryRequest {
        metric: "tasks_created".to_owned(),
        period: "week".to_owned(),
        ..QueryRequest::default()
    };
    let metric = query::check_request(&request)?;
    let engine = query::engine_query(&request, None, None)?;
    let input = AnalyticsInput::default();
    let facts = wg_analytics::build(&input);
    let answer = query::run_query(&input, &facts, "w1", &engine)?;
    let at = Timestamp::parse_iso("2026-10-26T10:00:00Z")?;
    let out = query::query_out(&request, &metric, &answer, at);
    assert_eq!(
        keys(&out),
        [
            "metric",
            "query",
            "rows",
            "sample_size",
            "provenance",
            "notes"
        ]
    );
    let echo = key(&out, "query").ok_or("no query echo")?;
    assert_eq!(
        keys(&echo),
        ["metric", "group_by", "period", "filters", "limit"]
    );
    let provenance = key(&out, "provenance").ok_or("no provenance")?;
    assert_eq!(
        key(&provenance, "generated_at"),
        Some(Value::Str(PyStr::from("2026-10-26T10:00:00+00:00")))
    );
    Ok(())
}

#[test]
fn query_view_prints_no_data_and_the_notes() -> TestResult {
    let request = QueryRequest {
        metric: "tasks_created".to_owned(),
        period: "week".to_owned(),
        ..QueryRequest::default()
    };
    let metric = query::check_request(&request)?;
    let engine = query::engine_query(&request, None, None)?;
    let input = AnalyticsInput::default();
    let facts = wg_analytics::build(&input);
    let mut answer = query::run_query(&input, &facts, "w1", &engine)?;
    answer.rows.clear();
    answer.sample_size = 0;
    let at = Timestamp::parse_iso("2026-10-26T10:00:00Z")?;
    let out = query::query_out(&request, &metric, &answer, at);
    let console = Console::new(100);
    let text = view::query_view(&console, &out);
    assert!(text.contains("no data"), "{text}");
    assert!(text.contains("note: "), "{text}");
    Ok(())
}

#[test]
fn bad_datetime_is_a_click_usage_error() -> TestResult {
    let root = args::command();
    let matches = root.try_get_matches_from([
        "analytics",
        "query",
        "tasks_created",
        "--since",
        "yesterday",
    ])?;
    let (_, sub) = matches.subcommand().ok_or("no subcommand")?;
    let Err(err) = args::query_request(sub) else {
        return Err("a bad datetime was accepted".into());
    };
    assert_eq!(err.exit_code(), 2);
    Ok(())
}

#[test]
fn pydantic_errors_list_both_fields_in_python_text() -> TestResult {
    let Err(err) = query::check_inputs("drop_table", "fortnight") else {
        return Err("both fields are wrong".into());
    };
    assert_eq!(err.code, ErrorCode::ValidationFailed);
    assert!(
        err.message.starts_with(
            "invalid input: metric: Value error, unknown metric 'drop_table'; known metrics: "
        ),
        "{}",
        err.message
    );
    assert!(
        err.message
            .ends_with("; period: Input should be 'day', 'week', 'month', 'quarter' or 'year'"),
        "{}",
        err.message
    );
    let errors = err.details.get("errors").and_then(|e| e.as_array());
    assert_eq!(errors.map(Vec::len), Some(2));
    Ok(())
}

#[test]
fn refused_dimension_carries_the_allowed_list() {
    let err = wg_analytics::core::query::QueryError::Dimension {
        metric: "tasks_created".to_owned(),
        dimension: "colour".to_owned(),
    };
    let refused = query::refusal(&err);
    assert_eq!(
        refused.message,
        "metric tasks_created cannot be grouped by 'colour'"
    );
    let allowed = refused.details.get("allowed").and_then(|a| a.as_array());
    let names: Vec<&str> = allowed
        .map(|list| list.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(
        names,
        [
            "agent",
            "category",
            "period",
            "priority",
            "project",
            "repository",
            "status"
        ]
    );
}

#[test]
fn compare_windows_are_the_naive_datetimes_typed() -> TestResult {
    let input = AnalyticsInput::default();
    let facts = wg_analytics::build(&input);
    let at = |day: u8| wg_json::LocalDateTime {
        year: 2026,
        month: 10,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        offset_seconds: None,
    };
    let (a, b) = ((at(1), at(8)), (at(8), at(15)));
    let window = |(since, until): (wg_json::LocalDateTime, wg_json::LocalDateTime)| {
        Ok::<_, wg_types::WgError>((query::instant(since)?, query::instant(until)?))
    };
    let core = wg_analytics::core::service::compare(
        &input,
        &facts,
        "w1",
        "tasks_created",
        window(a)?,
        window(b)?,
        &[],
    )
    .map_err(|e| e.to_string())?;
    let out = wg_analytics_feature::spec::analyses::compare_out(&core, a, b);
    assert_eq!(
        keys(&out),
        [
            "metric",
            "statistic",
            "period_a",
            "period_b",
            "rows",
            "notes"
        ]
    );
    assert_eq!(
        key(&out, "period_a"),
        Some(Value::List(vec![
            Value::LocalDateTime(a.0),
            Value::LocalDateTime(a.1)
        ]))
    );
    Ok(())
}

#[test]
fn fingerprint_is_the_first_32_hex_digits_of_sha256() {
    let digest = wg_analytics_feature::spec::analyses::fingerprint("retry_loop|x");
    assert_eq!(digest.len(), 32);
    assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn history_view_sums_churn_and_cuts_titles() {
    let row = Value::Dict(
        [
            ("key", Value::Str(PyStr::from("WG-1"))),
            ("category", Value::Null),
            ("attempts", Value::Int(2)),
            ("commits", Value::Int(3)),
            ("churn_insertions", Value::Int(10)),
            ("churn_deletions", Value::Int(5)),
            ("final_insertions", Value::Null),
            ("final_deletions", Value::Null),
            ("title", Value::Str(PyStr::from("x".repeat(80).as_str()))),
        ]
        .into_iter()
        .map(|(k, v)| (PyStr::from(k), v))
        .collect(),
    );
    let text = view::history_view(&Console::new(120), &Value::List(vec![row]));
    assert!(text.contains("WG-1"), "{text}");
    assert!(text.contains("15"), "{text}");
    assert!(text.contains(&"x".repeat(50)), "{text}");
    assert!(!text.contains(&"x".repeat(51)), "{text}");
}
