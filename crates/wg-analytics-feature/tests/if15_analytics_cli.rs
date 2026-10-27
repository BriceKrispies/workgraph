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
use wg_storecodec::Raw;
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
