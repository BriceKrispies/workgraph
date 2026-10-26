//! The query engine (`analytics/service.py` `build_sql` and the aggregation of `query`): a metric
//! over its base relation, filtered by the query's dimensions, period window and repository, and
//! grouped by dimensions and period buckets. The result rows have the keys and values the Python
//! service returns, including the `*_label` names of projects and repositories.
//!
//! The caller resolves the project and repository filters to ids first (the service does this
//! against the database); everything else is computed here.

use std::cmp::Ordering;
use std::fmt;

use rpds::VectorSync;
use serde_json::Value;
use wg_types::Timestamp;

use super::bases::{self, Base, Row, dimensions};
use super::metrics::{Metric, Ratio, Statistic, TimeColumn, find};
use super::period::{Period, bucket};
use super::stats::{add_opt, mean, quantile_cont, round6, sorted, sum};
use crate::Facts;
use crate::agg::Groups;
use crate::source::AnalyticsInput;

/// The filters of a query. Project and repository are ids.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filters {
    pub project: Option<String>,
    pub repository: Option<String>,
    pub category: Option<String>,
    pub agent: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub priority: Option<String>,
    pub status: Option<String>,
    pub kind: Option<String>,
    /// Rows whose metric time is at or after this instant.
    pub since: Option<Timestamp>,
    /// Rows whose metric time is before this instant.
    pub until: Option<Timestamp>,
}

/// A metric query. `group_by` names dimensions of the metric's base, or `period`.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub metric: String,
    pub group_by: Vec<String>,
    pub period: Period,
    pub filters: Filters,
    pub limit: usize,
}

impl Query {
    /// A query with the service defaults: no grouping, weekly periods and 500 rows.
    pub fn new(metric: &str) -> Self {
        Self {
            metric: metric.to_string(),
            group_by: Vec::new(),
            period: Period::Week,
            filters: Filters::default(),
            limit: 500,
        }
    }
}

/// Why a query is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    UnknownMetric(String),
    /// The metric's base has no such dimension.
    Dimension {
        metric: String,
        dimension: String,
    },
    /// The metric's base has no column for this filter.
    Filter {
        metric: String,
        column: String,
    },
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::UnknownMetric(metric) => write!(f, "unknown metric {metric:?}"),
            QueryError::Dimension { metric, dimension } => {
                write!(f, "metric {metric} cannot be grouped by {dimension:?}")
            }
            QueryError::Filter { metric, column } => {
                write!(f, "metric {metric} cannot be filtered by {column}")
            }
        }
    }
}

impl std::error::Error for QueryError {}

/// The result of a query: one JSON object per output row, and the number of rows the rows summarise
/// (the sum of their `n`).
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub rows: Vec<Value>,
    pub sample_size: i64,
}

/// The group key of an output row: one value per `group_by` entry, `None` for NULL.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupKey(Vec<Option<String>>);

impl Ord for GroupKey {
    /// Lexicographic, with NULL after every value, as `ORDER BY` sorts NULLs last.
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .iter()
            .zip(other.0.iter())
            .map(|(a, b)| match (a, b) {
                (Some(x), Some(y)) => x.cmp(y),
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
            })
            .find(|order| *order != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    }
}

impl PartialOrd for GroupKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The accumulated rows of one group.
#[derive(Debug, Clone)]
struct Acc {
    rows: i64,
    /// The values of the rows that have one, for the distribution and sum statistics.
    values: VectorSync<f64>,
    hits: i64,
    numerator: Option<f64>,
    denominator: Option<f64>,
}

impl Default for Acc {
    fn default() -> Self {
        Self {
            rows: 0,
            values: VectorSync::new_sync(),
            hits: 0,
            numerator: None,
            denominator: None,
        }
    }
}

impl Acc {
    fn add(self, row: &Row, metric: &Metric) -> Self {
        let values = match (metric.value)(row) {
            Some(value) => self.values.push_back(value),
            None => self.values,
        };
        let (hit, numerator, denominator) = match metric.ratio {
            Some(Ratio::Hits(hit)) => (i64::from(hit(row)), self.numerator, self.denominator),
            Some(Ratio::Sums {
                numerator: top,
                denominator: bottom,
            }) => (
                0,
                add_opt(self.numerator, top(row)),
                add_opt(self.denominator, bottom(row)),
            ),
            None => (0, self.numerator, self.denominator),
        };
        Self {
            rows: self.rows + 1,
            values,
            hits: self.hits + hit,
            numerator,
            denominator,
        }
    }

    /// The `n` of the output row: the rows, or for a distribution the values.
    fn sample(&self, metric: &Metric) -> i64 {
        match metric.statistic {
            Statistic::Distribution => self.values.len() as i64,
            _ => self.rows,
        }
    }
}

/// The display names of projects and repositories, for the `*_label` columns.
struct Labels {
    projects: Groups<String, String>,
    repositories: Groups<String, String>,
}

fn labels(input: &AnalyticsInput, workspace: &str) -> Labels {
    let in_workspace = |id: &Option<String>| id.is_some() && id.as_deref() == Some(workspace);
    Labels {
        projects: input
            .projects
            .iter()
            .filter(|project| in_workspace(&project.workspace_id))
            .fold(Groups::new_sync(), |map, project| {
                match (&project.id, &project.slug) {
                    (Some(id), Some(slug)) => map.insert(id.clone(), slug.clone()),
                    _ => map,
                }
            }),
        repositories: input
            .repositories
            .iter()
            .filter(|repository| in_workspace(&repository.workspace_id))
            .fold(Groups::new_sync(), |map, repository| {
                match (&repository.id, &repository.name) {
                    (Some(id), Some(name)) => map.insert(id.clone(), name.clone()),
                    _ => map,
                }
            }),
    }
}

/// The value of a dimension column of a row.
fn dimension_value<'a>(row: &'a Row, dimension: &str) -> Option<&'a str> {
    match dimension {
        "project" => row.project.as_deref(),
        "category" => row.category.as_deref(),
        "priority" => row.priority.as_deref(),
        "status" => row.status.as_deref(),
        "agent" => row.agent.as_deref(),
        "provider" => row.provider.as_deref(),
        "model" => row.model.as_deref(),
        "repository" => row.repository.as_deref(),
        "module" => row.module.as_deref(),
        "file_class" => row.file_class.as_deref(),
        "kind" => row.kind.as_deref(),
        "check_name" => row.check_name.as_deref(),
        "relation_type" => row.relation_type.as_deref(),
        _ => None,
    }
}

/// The time a metric's filters and period grouping read.
fn time_of(row: &Row, time: TimeColumn) -> Option<Timestamp> {
    match time {
        TimeColumn::CreatedAt => row.created_at,
        TimeColumn::CompletedAt => row.completed_at,
        TimeColumn::AcceptedAt => row.accepted_at,
        TimeColumn::StartedAt => row.started_at,
        TimeColumn::FinishedAt => row.finished_at,
        TimeColumn::CommittedAt => row.committed_at,
    }
}

/// The simple equality filters of a query: the column and the value, for each one set.
fn equality_filters(filters: &Filters) -> Vec<(&'static str, &str)> {
    [
        ("project", filters.project.as_deref()),
        ("category", filters.category.as_deref()),
        ("priority", filters.priority.as_deref()),
        ("status", filters.status.as_deref()),
        ("model", filters.model.as_deref()),
        ("kind", filters.kind.as_deref()),
        ("provider", filters.provider.as_deref()),
        ("agent", filters.agent.as_deref()),
    ]
    .into_iter()
    .filter_map(|(column, value)| value.map(|value| (column, value)))
    .collect()
}

/// The rows of a task that the join with `task_repositories` produces when a query groups by
/// repository: one row per linked repository, and none for a task without links.
fn expand_repositories(row: Row, expand: bool) -> Vec<Row> {
    if expand {
        row.repositories
            .iter()
            .map(|repository| Row {
                repository: Some(repository.clone()),
                ..row.clone()
            })
            .collect()
    } else {
        vec![row]
    }
}

/// Whether a row's time is inside the query window.
fn in_window(at: Option<Timestamp>, filters: &Filters) -> bool {
    filters
        .since
        .is_none_or(|since| at.is_some_and(|at| at >= since))
        && filters
            .until
            .is_none_or(|until| at.is_some_and(|at| at < until))
}

fn group_key(row: &Row, query: &Query, time: TimeColumn) -> GroupKey {
    GroupKey(
        query
            .group_by
            .iter()
            .map(|dimension| {
                if dimension == "period" {
                    time_of(row, time).map(|at| bucket(query.period, at))
                } else {
                    dimension_value(row, dimension).map(str::to_string)
                }
            })
            .collect(),
    )
}

fn int(value: i64) -> Value {
    Value::from(value)
}

fn float(value: Option<f64>) -> Value {
    value.map_or(Value::Null, |value| Value::from(round6(value)))
}

/// An aggregate of the values of a metric: `MIN`, `MAX` and `SUM` of an integer column stay
/// integers in the Python service, every other aggregate is a float.
fn aggregate_value(metric: &Metric, value: Option<f64>) -> Value {
    match value {
        Some(value) if metric.integer => int(value as i64),
        value => float(value),
    }
}

/// The statistic columns of one group, in the order of the Python service.
fn statistics(metric: &Metric, acc: &Acc) -> Vec<(&'static str, Value)> {
    match metric.statistic {
        Statistic::Count => vec![("value", int(acc.rows)), ("n", int(acc.rows))],
        Statistic::Sum => {
            let values: Vec<f64> = acc.values.iter().copied().collect();
            vec![
                ("value", aggregate_value(metric, sum(&values))),
                ("n", int(acc.rows)),
            ]
        }
        Statistic::Ratio => match metric.ratio {
            Some(Ratio::Hits(_)) => vec![
                ("numerator", int(acc.hits)),
                ("denominator", int(acc.rows)),
                (
                    "value",
                    float((acc.rows > 0).then(|| acc.hits as f64 / acc.rows as f64)),
                ),
                ("n", int(acc.rows)),
            ],
            _ => vec![
                ("numerator", float(acc.numerator)),
                ("denominator", float(acc.denominator)),
                (
                    "value",
                    float(
                        acc.numerator
                            .zip(acc.denominator)
                            .filter(|(_, denominator)| *denominator != 0.0)
                            .map(|(numerator, denominator)| numerator / denominator),
                    ),
                ),
                ("n", int(acc.rows)),
            ],
        },
        Statistic::Distribution => {
            let values = sorted(&acc.values);
            vec![
                ("n", int(values.len() as i64)),
                ("mean", float(mean(&values))),
                ("median", float(quantile_cont(&values, 0.5))),
                ("p75", float(quantile_cont(&values, 0.75))),
                ("p90", float(quantile_cont(&values, 0.9))),
                ("p95", float(quantile_cont(&values, 0.95))),
                ("min", aggregate_value(metric, values.first().copied())),
                ("max", aggregate_value(metric, values.last().copied())),
                ("sum", aggregate_value(metric, sum(&values))),
            ]
        }
    }
}

fn output_row(query: &Query, labels: &Labels, metric: &Metric, key: &GroupKey, acc: &Acc) -> Value {
    let dimensions = query
        .group_by
        .iter()
        .zip(key.0.iter())
        .map(|(dimension, value)| {
            (
                dimension.clone(),
                value.clone().map_or(Value::Null, Value::String),
            )
        });
    let label_columns = query
        .group_by
        .iter()
        .zip(key.0.iter())
        .filter_map(|(dimension, value)| {
            let id = value.as_ref()?;
            let names = match dimension.as_str() {
                "project" => &labels.projects,
                "repository" => &labels.repositories,
                _ => return None,
            };
            let name = names.get(id).cloned().unwrap_or_else(|| id.clone());
            Some((format!("{dimension}_label"), Value::String(name)))
        });
    let statistics = statistics(metric, acc)
        .into_iter()
        .map(|(name, value)| (name.to_string(), value));
    Value::Object(dimensions.chain(label_columns).chain(statistics).collect())
}

/// Runs a query over the warehouse of one workspace.
pub fn run(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    query: &Query,
) -> Result<Answer, QueryError> {
    let metric =
        find(&query.metric).ok_or_else(|| QueryError::UnknownMetric(query.metric.clone()))?;
    let allowed = dimensions(metric.base);
    let refuse_dimension = |dimension: &String| QueryError::Dimension {
        metric: metric.id.to_string(),
        dimension: dimension.clone(),
    };
    query
        .group_by
        .iter()
        .filter(|dimension| dimension.as_str() != "period")
        .filter(|dimension| !allowed.contains(&dimension.as_str()))
        .map(refuse_dimension)
        .next()
        .map_or(Ok(()), Err)?;

    let simple = equality_filters(&query.filters);
    let filtered_columns = simple
        .iter()
        .map(|(column, _)| *column)
        .chain(query.filters.repository.as_ref().map(|_| "repository"));
    filtered_columns
        .filter(|column| !allowed.contains(column))
        .map(|column| QueryError::Filter {
            metric: metric.id.to_string(),
            column: column.to_string(),
        })
        .next()
        .map_or(Ok(()), Err)?;

    let grouped_by_repository = query
        .group_by
        .iter()
        .any(|dimension| dimension == "repository");
    // A task is counted once per linked repository only when the query groups by repository.
    let expand = metric.base == Base::Tasks && grouped_by_repository;
    let repository_matches = |row: &Row, wanted: &str| {
        if metric.base == Base::Tasks && !grouped_by_repository {
            row.repositories
                .iter()
                .any(|repository| repository.as_str() == wanted)
        } else {
            row.repository.as_deref() == Some(wanted)
        }
    };

    let rows = bases::rows(metric.base, input, facts)
        .into_iter()
        .filter(|row| row.ws.as_deref() == Some(workspace))
        .flat_map(|row| expand_repositories(row, expand))
        .filter(|row| (metric.filter)(row))
        .filter(|row| in_window(time_of(row, metric.time), &query.filters))
        .filter(|row| {
            simple
                .iter()
                .all(|(column, value)| dimension_value(row, column) == Some(*value))
        })
        .filter(|row| {
            query
                .filters
                .repository
                .as_deref()
                .is_none_or(|wanted| repository_matches(row, wanted))
        });

    let groups = rows.fold(Groups::<GroupKey, Acc>::new_sync(), |groups, row| {
        let key = group_key(&row, query, metric.time);
        let acc = groups
            .get(&key)
            .cloned()
            .unwrap_or_default()
            .add(&row, &metric);
        groups.insert(key, acc)
    });
    // An ungrouped aggregate over no rows is still one row (`SELECT COUNT(*) FROM ...`).
    let groups = if groups.is_empty() && query.group_by.is_empty() {
        groups.insert(GroupKey(Vec::new()), Acc::default())
    } else {
        groups
    };

    let labels = labels(input, workspace);
    let outputs: Vec<(GroupKey, Acc)> = groups
        .iter()
        .take(query.limit)
        .map(|(key, acc)| (key.clone(), acc.clone()))
        .collect();
    let sample_size = outputs.iter().map(|(_, acc)| acc.sample(&metric)).sum();
    let rows = outputs
        .iter()
        .map(|(key, acc)| output_row(query, &labels, &metric, key, acc))
        .collect();
    Ok(Answer { rows, sample_size })
}
