//! GV-15, metrics: every case of `golden/analytics/metrics-*.json` (a metric query, the sample size
//! and the rows the Python service returned from DuckDB) must be answered by the pure query engine
//! with the same rows, as multisets. The registry must list the metrics and periods of
//! `definitions.json` in the same order.

use crate::common;

use common::{golden, input, records, text};
use serde_json::Value;
use wg_analytics::build;
use wg_analytics::core::metrics::registry;
use wg_analytics::core::period::Period;
use wg_analytics::core::query::{Filters, Query, run};
use wg_types::Timestamp;

/// The id of the row of `table` whose `key` column is `value` in workspace `ws`.
fn id_of(source: &Value, table: &str, key: &str, value: &str, ws: &str) -> String {
    records(source, table)
        .iter()
        .find(|row| {
            text(row, key).as_deref() == Some(value)
                && text(row, "workspace_id").as_deref() == Some(ws)
        })
        .and_then(|row| text(row, "id"))
        .unwrap_or_else(|| panic!("no {table} row with {key} {value}"))
}

/// The query of one golden case. The golden filters name projects by slug and repositories by
/// name; the service resolves them to ids before the engine runs, and so does this.
fn query_of(case: &Value, source: &Value, ws: &str) -> Query {
    let query = &case["query"];
    let filter = |key: &str| query["filters"][key].as_str().map(str::to_string);
    let time = |key: &str| {
        filter(key).map(|moment| {
            Timestamp::parse_iso(&moment)
                .unwrap_or_else(|err| panic!("golden timestamp {moment}: {err}"))
        })
    };
    Query {
        metric: query["metric"]
            .as_str()
            .unwrap_or_else(|| panic!("metric"))
            .to_string(),
        group_by: query["group_by"]
            .as_array()
            .unwrap_or_else(|| panic!("group_by"))
            .iter()
            .map(|dimension| {
                dimension
                    .as_str()
                    .unwrap_or_else(|| panic!("dimension"))
                    .to_string()
            })
            .collect(),
        period: Period::parse(query["period"].as_str().unwrap_or_else(|| panic!("period")))
            .unwrap_or_else(|| panic!("period name")),
        filters: Filters {
            project: filter("project").map(|slug| id_of(source, "projects", "slug", &slug, ws)),
            repository: filter("repository")
                .map(|name| id_of(source, "repositories", "name", &name, ws)),
            category: filter("category"),
            agent: filter("agent"),
            provider: filter("provider"),
            model: filter("model"),
            priority: filter("priority"),
            status: filter("status"),
            kind: filter("kind"),
            since: time("since"),
            until: time("until"),
        },
        limit: query["limit"].as_u64().unwrap_or_else(|| panic!("limit")) as usize,
    }
}

/// Numbers agree to the rounding the golden already applied; everything else is exact.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-9,
        _ => a == b,
    }
}

fn same_row(a: &Value, b: &Value) -> bool {
    match (a.as_object(), b.as_object()) {
        (Some(left), Some(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| {
                    right.get(key).is_some_and(|other| same_value(value, other))
                })
        }
        _ => false,
    }
}

fn assert_same_rows(query: &Query, actual: &[Value], expected: &[Value]) {
    let mut unmatched: Vec<&Value> = actual.iter().collect();
    for row in expected {
        let position = unmatched
            .iter()
            .position(|candidate| same_row(candidate, row))
            .unwrap_or_else(|| panic!("{query:?}: no row equals the golden row {row}"));
        unmatched.remove(position);
    }
    assert!(
        unmatched.is_empty(),
        "{query:?}: rows without a golden row: {unmatched:?}"
    );
}

fn check_metrics(name: &str) {
    let source = golden(&format!("input-{name}.json"));
    let expected = golden(&format!("metrics-{name}.json"));
    let warehouse = input(&source);
    let facts = build(&warehouse);
    let ws =
        text(&records(&source, "workspaces")[0], "id").unwrap_or_else(|| panic!("workspace id"));

    let cases = expected["cases"]
        .as_array()
        .unwrap_or_else(|| panic!("cases"));
    for case in cases {
        let query = query_of(case, &source, &ws);
        let result = run(&warehouse, &facts, &ws, &query);
        match case.get("error") {
            Some(error) => assert!(result.is_err(), "{query:?} should be refused ({error})"),
            None => {
                let answer = result.unwrap_or_else(|err| panic!("{query:?}: {err}"));
                assert_eq!(
                    answer.sample_size,
                    case["sample_size"]
                        .as_i64()
                        .unwrap_or_else(|| panic!("sample_size")),
                    "{query:?}"
                );
                assert_same_rows(
                    &query,
                    &answer.rows,
                    case["rows"].as_array().unwrap_or_else(|| panic!("rows")),
                );
            }
        }
    }
    assert!(!cases.is_empty());
}

// covers: C012
#[test]
fn gv15_demo_metrics_match_the_python_service() {
    check_metrics("demo");
}

// covers: C012
#[test]
fn gv15_generated_metrics_match_the_python_service() {
    check_metrics("gen");
}

// covers: C010
#[test]
fn gv15_metric_registry_matches_the_python_catalog() {
    let definitions = golden("definitions.json");
    let ids: Vec<&str> = registry().iter().map(|metric| metric.id).collect();
    let expected_ids: Vec<&str> = definitions["metrics"]
        .as_array()
        .unwrap_or_else(|| panic!("metrics"))
        .iter()
        .map(|id| id.as_str().unwrap_or_else(|| panic!("metric id")))
        .collect();
    assert_eq!(ids, expected_ids);

    let periods: Vec<&str> = Period::ALL.iter().map(|period| period.name()).collect();
    let expected_periods: Vec<&str> = definitions["periods"]
        .as_array()
        .unwrap_or_else(|| panic!("periods"))
        .iter()
        .map(|period| period.as_str().unwrap_or_else(|| panic!("period name")))
        .collect();
    assert_eq!(periods, expected_periods);
}
