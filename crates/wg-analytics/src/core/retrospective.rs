//! The retrospective of `analytics/retrospective.py`: the measured facts of a window, and the
//! deterministic report written from them. Every number comes from this crate's metric queries,
//! coupling report, failure report and task history. The model narrative and the stored report are
//! the shell's (`wg-analytics-feature`); this module gives it the facts document, the highlights and
//! the opportunities the narrative is written from.
//!
//! Python formats the report with f-strings, so the values are written as Python's `str()` writes
//! them (see [`py_str`]), and a missing key reads as `None` where Python's `dict.get` would.

use serde_json::{Map, Value, json};
use wg_pynum::stable_sort_by;
use wg_telemetry::json::py_str;

use super::coupling;
use super::failures;
use super::query::{Filters, Query, QueryError, run};
use super::service::{Window, task_history};
use crate::Facts;
use crate::source::AnalyticsInput;

/// The tasks the history reads for the period and the significant changes (`limit=200`).
const HISTORY_LIMIT: usize = 200;

/// The window and the scope of one retrospective. `project` and `repository` are the names as the
/// caller typed them (they appear in the facts); the `*_id` fields are the same scope resolved to
/// ids, which the metric filters use. `generated_at` is the failure report's generation instant.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub window: Window,
    pub project: Option<&'a str>,
    pub repository: Option<&'a str>,
    pub project_id: Option<&'a str>,
    pub repository_id: Option<&'a str>,
    pub generated_at: &'a str,
}

/// The retrospective of one window: the title, the facts, the deterministic content, the highlights
/// and the opportunities the narrative starts from.
#[derive(Debug, Clone, PartialEq)]
pub struct Retrospective {
    pub title: String,
    pub facts: Value,
    pub content: String,
    pub highlights: Vec<String>,
    pub opportunities: Vec<String>,
}

/// `generate_retrospective`'s measured part, over the warehouse of `workspace`.
pub fn retrospective(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    request: &Request<'_>,
    fingerprint: &impl Fn(&str) -> String,
) -> Result<Retrospective, QueryError> {
    let (since, until) = request.window;
    let scope = |scoped: bool| Filters {
        project: request.project_id.map(str::to_owned),
        repository: request.repository_id.map(str::to_owned),
        since: scoped.then_some(since),
        until: scoped.then_some(until),
        ..Filters::default()
    };
    let rows = |metric: &str, group_by: &[&str], scoped: bool| -> Result<Vec<Value>, QueryError> {
        let query = Query {
            group_by: group_by
                .iter()
                .map(|dimension| dimension.to_string())
                .collect(),
            filters: scope(scoped),
            ..Query::new(metric)
        };
        run(input, facts, workspace, &query).map(|answer| answer.rows)
    };
    let accepted = rows("tasks_accepted", &[], true)?;
    let by_category = rows("tasks_accepted", &["category"], true)?;
    let completed = rows("tasks_completed", &[], true)?;
    let created = rows("tasks_created", &[], true)?;
    let elapsed = rows("task_elapsed", &[], true)?;
    let agents = rows("agent_completion_rate", &["provider", "model"], true)?;
    let churn = rows("cumulative_churn", &[], true)?;
    let final_change = rows("final_change_size", &[], true)?;
    let rework = rows("rework_ratio", &[], true)?;
    let verification = rows("verification_pass_rate", &[], true)?;
    let failing = rows("check_failures", &["check_name"], true)?;
    let blocked = rows("blocked_tasks", &[], false)?;
    let coupled = coupling::coupling_report(input, facts, workspace, 1, request.project);
    let report = failures::failures(input, facts, workspace, request.generated_at, fingerprint);
    let findings: Vec<&Value> = array_of(report.get("findings"));
    let recent = task_history(
        input,
        facts,
        workspace,
        None,
        request.repository_id,
        HISTORY_LIMIT,
    );
    let since_text = since.format_isoformat();
    let until_text = until.format_isoformat();
    let in_period: Vec<&Value> = array_of(Some(&recent))
        .into_iter()
        .filter(|task| {
            task.get("accepted_at")
                .and_then(Value::as_str)
                .is_some_and(|at| {
                    let at = prefix19(at);
                    prefix19(&since_text) <= at && at <= prefix19(&until_text)
                })
        })
        .collect();
    let significant: Vec<&Value> =
        stable_sort_by(in_period, |a, b| lines_changed(b).cmp(&lines_changed(a)))
            .into_iter()
            .take(5)
            .collect();

    let summary = coupled.get("summary").cloned().unwrap_or(Value::Null);
    let pairs: Vec<&Value> = array_of(coupled.get("commonly_changed_pairs"))
        .into_iter()
        .take(5)
        .collect();
    let elapsed_row = first_row(&elapsed);
    let rework_row = first_row(&rework);
    let verification_row = first_row(&verification);
    // A repeated category keeps its first position and takes the later value, as a Python dict does.
    let categories: Map<String, Value> = by_category
        .iter()
        .map(|row| {
            (
                category_of(row),
                row.get("value").cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    let tasks_created = count_of(&created, "value");
    let tasks_completed = count_of(&completed, "value");
    let tasks_accepted = count_of(&accepted, "value");
    let blocked_now = count_of(&blocked, "value");
    let since_date = since.to_offset_datetime().date().to_string();
    let until_date = until.to_offset_datetime().date().to_string();
    let title = format!("Retrospective {since_date} - {until_date}");

    let facts_doc = json!({
        "period": {
            "since": since.format_isoformat(),
            "until": until.format_isoformat(),
            "project": request.project,
            "repository": request.repository,
        },
        "tasks": {
            "created": tasks_created,
            "completed": tasks_completed,
            "accepted": tasks_accepted,
            "blocked_now": blocked_now,
        },
        "categories": Value::Object(categories),
        "elapsed_ms": elapsed_row.clone(),
        "agents": agents.clone(),
        "code": {
            "cumulative_churn": first_row(&churn),
            "final_change": first_row(&final_change),
            "rework_ratio": rework_row.clone(),
        },
        "verification": verification_row.clone(),
        "top_failing_checks": failing.iter().take(5).cloned().collect::<Vec<Value>>(),
        "coordination": summary.clone(),
        "coupled_pairs": pairs.iter().map(|pair| (*pair).clone()).collect::<Vec<Value>>(),
        "findings": findings
            .iter()
            .take(10)
            .map(|finding| json!({
                "title": finding.get("title").cloned().unwrap_or(Value::Null),
                "kind": finding.get("kind").cloned().unwrap_or(Value::Null),
                "evidence": finding.get("evidence").cloned().unwrap_or(Value::Null),
            }))
            .collect::<Vec<Value>>(),
        "significant_changes": significant
            .iter()
            .map(|task| json!({
                "task": task.get("key").cloned().unwrap_or(Value::Null),
                "title": task.get("title").cloned().unwrap_or(Value::Null),
                "final_lines": lines_changed(task),
            }))
            .collect::<Vec<Value>>(),
    });
    let opportunities: Vec<String> = findings
        .iter()
        .filter(|finding| {
            finding
                .get("severity")
                .and_then(Value::as_str)
                .is_some_and(|severity| severity == "warning" || severity == "error")
        })
        .map(|finding| text_of(finding.get("title")))
        .take(5)
        .collect();
    let lines = report_lines(&ReportParts {
        since_date: &since_date,
        until_date: &until_date,
        created: &tasks_created,
        completed: &tasks_completed,
        accepted: &tasks_accepted,
        blocked_now: &blocked_now,
        elapsed: &elapsed_row,
        by_category: &by_category,
        findings: &findings,
        rework: &rework_row,
        agents: &agents,
        summary: &summary,
        pairs: &pairs,
        verification: &verification_row,
        failing: &failing,
        significant: &significant,
        opportunities: &opportunities,
    });
    let highlights: Vec<String> = lines.iter().skip(6).take(3).cloned().collect();
    Ok(Retrospective {
        title,
        facts: facts_doc,
        content: lines.join("\n") + "\n",
        highlights,
        opportunities,
    })
}

/// The inputs of the deterministic report's lines.
struct ReportParts<'a> {
    since_date: &'a str,
    until_date: &'a str,
    created: &'a Value,
    completed: &'a Value,
    accepted: &'a Value,
    blocked_now: &'a Value,
    elapsed: &'a Value,
    by_category: &'a [Value],
    findings: &'a [&'a Value],
    rework: &'a Value,
    agents: &'a [Value],
    summary: &'a Value,
    pairs: &'a [&'a Value],
    verification: &'a Value,
    failing: &'a [Value],
    significant: &'a [&'a Value],
    opportunities: &'a [String],
}

/// The lines of the deterministic report, section by section, as `retrospective.py` writes them.
fn report_lines(parts: &ReportParts<'_>) -> Vec<String> {
    let header = [
        format!("# Retrospective: {} to {}", parts.since_date, parts.until_date),
        String::new(),
        "_All numbers are measured from Workgraph records; see the evidence references in brackets._"
            .to_owned(),
        String::new(),
        "## Work completed".to_owned(),
        String::new(),
        format!(
            "- Tasks created: **{}**, completion reported: **{}**, accepted: **{}**.",
            py_str(parts.created),
            py_str(parts.completed),
            py_str(parts.accepted)
        ),
        format!(
            "- Median time from creation to acceptance: **{}** (n={}).",
            fmt_ms(parts.elapsed.get("median")),
            parts.elapsed.get("n").map_or("0".to_owned(), py_str)
        ),
        String::new(),
        "## Category breakdown".to_owned(),
        String::new(),
    ];
    let categories = or_none(
        parts
            .by_category
            .iter()
            .map(|row| format!("- {}: {}", category_of(row), text_field(row, "value")))
            .collect(),
        "- no accepted tasks in period",
    );
    let bottlenecks = [
        String::new(),
        "## Bottlenecks".to_owned(),
        String::new(),
        format!("- Currently blocked tasks: {}.", py_str(parts.blocked_now)),
    ];
    let dependency = parts
        .findings
        .iter()
        .filter(|finding| {
            finding.get("kind").and_then(Value::as_str) == Some("dependency_bottleneck")
        })
        .map(|finding| finding_line(finding));
    let rework_section = [String::new(), "## Rework".to_owned(), String::new()];
    let rework_text = if truthy(parts.rework) {
        parts.rework.get("median").map_or("n/a".to_owned(), py_str)
    } else {
        "n/a".to_owned()
    };
    let rework_count = if truthy(parts.rework) {
        parts.rework.get("n").map_or("0".to_owned(), py_str)
    } else {
        "0".to_owned()
    };
    let rework_line =
        format!("- Median rework ratio (churn / final change): {rework_text} (n={rework_count}).");
    let rework_findings = parts
        .findings
        .iter()
        .filter(|finding| {
            matches!(
                finding.get("kind").and_then(Value::as_str),
                Some("high_rework" | "retry_loop")
            )
        })
        .map(|finding| finding_line(finding));
    let agents_section = [
        String::new(),
        "## Agent effectiveness".to_owned(),
        String::new(),
    ];
    let agents = or_none(
        parts.agents.iter().map(agent_line).collect(),
        "- no agent attempts finished in the period",
    );
    let coordination = [
        String::new(),
        "## Cross-repository coordination".to_owned(),
        String::new(),
        format!(
            "- Cross-repository tasks: {} of {} with code; average repositories per task {}.",
            text_field(parts.summary, "cross_repository_tasks"),
            text_field(parts.summary, "tasks_with_code"),
            text_field(parts.summary, "average_repositories_per_task")
        ),
    ];
    let pair_lines = parts.pairs.iter().map(|pair| {
        let names = array_of(pair.get("pair"));
        format!(
            "- {} + {} changed together in {} task(s).",
            names.first().map_or("None".to_owned(), |name| py_str(name)),
            names.get(1).map_or("None".to_owned(), |name| py_str(name)),
            text_field(pair, "co_changes")
        )
    });
    let quality_head = [String::new(), "## Quality".to_owned(), String::new()];
    let verification = truthy(parts.verification).then(|| {
        format!(
            "- Verification pass rate: {}/{} runs passed.",
            text_field(parts.verification, "numerator"),
            text_field(parts.verification, "denominator")
        )
    });
    let checks = parts.failing.iter().take(5).map(|check| {
        format!(
            "- Check '{}' failed {} time(s).",
            text_field(check, "check_name"),
            text_field(check, "value")
        )
    });
    let significant_head = [
        String::new(),
        "## Significant changes".to_owned(),
        String::new(),
    ];
    let significant = or_none(
        parts
            .significant
            .iter()
            .map(|task| {
                format!(
                    "- [{}] {} ({} final lines)",
                    text_field(task, "key"),
                    text_field(task, "title"),
                    lines_changed(task)
                )
            })
            .collect(),
        "- none recorded",
    );
    let improvement = [
        String::new(),
        "## Potential improvement opportunities".to_owned(),
        String::new(),
    ];
    let investigate = or_none(
        parts
            .opportunities
            .iter()
            .map(|opportunity| format!("- Investigate: {opportunity}"))
            .collect(),
        "- No significant failure patterns detected.",
    );
    header
        .into_iter()
        .chain(categories)
        .chain(bottlenecks)
        .chain(dependency)
        .chain(rework_section)
        .chain(std::iter::once(rework_line))
        .chain(rework_findings)
        .chain(agents_section)
        .chain(agents)
        .chain(coordination)
        .chain(pair_lines)
        .chain(quality_head)
        .chain(verification)
        .chain(checks)
        .chain(significant_head)
        .chain(significant)
        .chain(improvement)
        .chain(investigate)
        .collect()
}

/// `- {title} [{tasks}]` for a finding: the first five task keys of its evidence.
fn finding_line(finding: &Value) -> String {
    let tasks: Vec<String> = array_of(
        finding
            .get("evidence")
            .and_then(|evidence| evidence.get("tasks")),
    )
    .into_iter()
    .take(5)
    .map(py_str)
    .collect();
    format!("- {} [{}]", text_of(finding.get("title")), tasks.join(", "))
}

/// One agent line: the success rate of a provider and model, or that it finished no attempt.
fn agent_line(agent: &Value) -> String {
    let provider = text_field(agent, "provider");
    match agent.get("value").and_then(Value::as_f64) {
        Some(rate) => format!(
            "- {provider} / {}: {}/{} attempts succeeded ({:.0}%)",
            text_field(agent, "model"),
            text_field(agent, "numerator"),
            text_field(agent, "denominator"),
            rate * 100.0
        ),
        None => format!("- {provider}: no finished attempts"),
    }
}

/// `_fmt_ms`: a duration in milliseconds as seconds, minutes or hours; `n/a` when there is none.
fn fmt_ms(value: Option<&Value>) -> String {
    match value.and_then(Value::as_f64) {
        None => "n/a".to_owned(),
        Some(milliseconds) => {
            let seconds = milliseconds / 1000.0;
            if seconds < 120.0 {
                format!("{seconds:.0}s")
            } else if seconds < 7200.0 {
                format!("{:.0}m", seconds / 60.0)
            } else {
                format!("{:.1}h", seconds / 3600.0)
            }
        }
    }
}

/// The lines of a list, or the placeholder line when the list is empty (`[...] or [placeholder]`).
fn or_none(lines: Vec<String>, placeholder: &str) -> Vec<String> {
    if lines.is_empty() {
        vec![placeholder.to_owned()]
    } else {
        lines
    }
}

/// The category of a row of `tasks_accepted` by category: `category or "unclassified"`.
fn category_of(row: &Value) -> String {
    match row.get("category") {
        Some(value) if truthy(value) => py_str(value),
        _ => "unclassified".to_owned(),
    }
}

/// `str(row.get(field))`: a missing key reads as `None`.
fn text_field(row: &Value, field: &str) -> String {
    text_of(row.get(field))
}

/// `str(value)` of an optional value; a missing value reads as `None`.
fn text_of(value: Option<&Value>) -> String {
    value.map_or("None".to_owned(), py_str)
}

/// Python truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(entries) => !entries.is_empty(),
    }
}

/// The first row of a query, or the empty object `{}` when there is none.
fn first_row(rows: &[Value]) -> Value {
    rows.first().cloned().unwrap_or_else(|| json!({}))
}

/// `rows[0].get(key) or 0`: the first row's value, or `0` when there is none or it is falsy.
fn count_of(rows: &[Value], key: &str) -> Value {
    rows.first()
        .and_then(|row| row.get(key))
        .filter(|value| truthy(value))
        .cloned()
        .unwrap_or_else(|| json!(0))
}

/// `final_insertions + final_deletions`, with a NULL counted as zero.
fn lines_changed(task: &Value) -> i64 {
    let count = |field: &str| task.get(field).and_then(Value::as_i64).unwrap_or(0);
    count("final_insertions") + count("final_deletions")
}

/// The first 19 characters of an ISO timestamp (the seconds), which the period comparison uses.
fn prefix19(at: &str) -> &str {
    at.get(..19).unwrap_or(at)
}

/// The items of a JSON array, or none.
fn array_of(value: Option<&Value>) -> Vec<&Value> {
    match value {
        Some(Value::Array(items)) => items.iter().collect(),
        _ => Vec::new(),
    }
}
