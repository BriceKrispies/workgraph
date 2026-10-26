//! The MCP output shape of every tool (CF-03, M01-M69).
//!
//! A tool returns a DTO, a dict wrapping a list (`{"events": [...]}`), or a dict of its own. This
//! module fixes all three in one table, [`output_shape`], so that each of the 69 tools has exactly
//! one wire type, and the tests can check every recorded transcript response against it:
//!
//! * a DTO is [`Shape::Dto`], built from the DTO module's own `FIELDS` constant (keys and order),
//!   with the nested DTOs it carries;
//! * a wrapper or a dict is [`Shape::Object`], with its keys in the order Python builds them;
//! * a value the table does not look into (a free-form JSON column, an open dict) is
//!   [`Shape::Any`].
//!
//! [`Shape::check`] compares a JSON value (with object key order preserved) against a shape and
//! names the first difference. The builders below produce the wrappers that no area module owns;
//! the area modules own the rest (`experiment_shapes`, `analytics_mcp_shapes`, `mcp_shapes`).

use serde_json::Value as Json;
use wg_json::{PyStr, Value};

use super::{
    agent_time_out, analytics_overview, analytics_query, analytics_query_out, attempt,
    classification_job, classification_out, classification_task, comment, commit_link,
    coupling_out, dependency_state, estimate_out, event, experiment, failures_out,
    metric_definition, page, predictor, relation, search_result, stack, stack_item,
    stack_repository, subagents_out, task, task_changes, task_detail, task_diff, task_file,
    task_history_row, task_rollup, trial,
};
use crate::convention::{counts, dict, int, optional_text, text, texts};

/// What a tool puts on the wire.
#[derive(Debug, Clone, Copy)]
pub enum Shape {
    /// Any JSON value: the table does not look inside.
    Any,
    /// `null` or the shape.
    Nullable(&'static Shape),
    /// An array whose every element has the shape.
    List(&'static Shape),
    /// An object with exactly these keys in this order, each value checked against its shape.
    Object(&'static [(&'static str, Shape)]),
    /// A DTO: exactly the keys of its `FIELDS` in order. `nested` names the fields whose values
    /// are checked against a shape; the other fields are not looked into.
    Dto {
        fields: &'static [&'static str],
        nested: &'static [(&'static str, Shape)],
    },
    /// The value has one of the shapes (a tool whose output depends on an argument).
    OneOf(&'static [Shape]),
}

impl Shape {
    /// Compare `value` with the shape; `Err` names the path of the first difference.
    pub fn check(&self, value: &Json) -> Result<(), String> {
        self.check_at(value, "$")
    }

    fn check_at(&self, value: &Json, path: &str) -> Result<(), String> {
        match self {
            Shape::Any => Ok(()),
            Shape::Nullable(inner) => {
                if value.is_null() {
                    Ok(())
                } else {
                    inner.check_at(value, path)
                }
            }
            Shape::List(inner) => {
                let items = value
                    .as_array()
                    .ok_or_else(|| format!("{path}: expected an array"))?;
                items
                    .iter()
                    .enumerate()
                    .try_for_each(|(index, item)| inner.check_at(item, &format!("{path}[{index}]")))
            }
            Shape::Object(entries) => {
                let object = value
                    .as_object()
                    .ok_or_else(|| format!("{path}: expected an object"))?;
                let keys: Vec<&str> = entries.iter().map(|(key, _)| *key).collect();
                compare_keys(path, &keys, object)?;
                entries
                    .iter()
                    .try_for_each(|(key, shape)| check_member(shape, object, key, path))
            }
            Shape::Dto { fields, nested } => {
                let object = value
                    .as_object()
                    .ok_or_else(|| format!("{path}: expected an object"))?;
                compare_keys(path, fields, object)?;
                nested
                    .iter()
                    .try_for_each(|(key, shape)| check_member(shape, object, key, path))
            }
            Shape::OneOf(shapes) => {
                let mut failures = Vec::new();
                for shape in *shapes {
                    match shape.check_at(value, path) {
                        Ok(()) => return Ok(()),
                        Err(failure) => failures.push(failure),
                    }
                }
                Err(format!("no variant matches: {}", failures.join(" | ")))
            }
        }
    }
}

fn check_member(
    shape: &Shape,
    object: &serde_json::Map<String, Json>,
    key: &str,
    path: &str,
) -> Result<(), String> {
    let member = object
        .get(key)
        .ok_or_else(|| format!("{path}: no key {key}"))?;
    shape.check_at(member, &format!("{path}.{key}"))
}

fn compare_keys(
    path: &str,
    expected: &[&str],
    object: &serde_json::Map<String, Json>,
) -> Result<(), String> {
    let got: Vec<&str> = object.keys().map(String::as_str).collect();
    if got == expected {
        Ok(())
    } else {
        Err(format!("{path}: keys {got:?}, expected {expected:?}"))
    }
}

const fn dto(fields: &'static [&'static str]) -> Shape {
    Shape::Dto {
        fields,
        nested: &[],
    }
}

const fn list(item: &'static Shape) -> Shape {
    Shape::List(item)
}

const fn nullable(inner: &'static Shape) -> Shape {
    Shape::Nullable(inner)
}

// Tasks.
const TASK: Shape = Shape::Dto {
    fields: &task::FIELDS,
    nested: &[("files", list(&dto(&task_file::FIELDS)))],
};
const COMMENT: Shape = dto(&comment::FIELDS);
const EVENT: Shape = dto(&event::FIELDS);
const PAGE_OF_TASKS: Shape = Shape::Dto {
    fields: &page::FIELDS,
    nested: &[("items", list(&TASK))],
};
const ATTEMPT: Shape = dto(&attempt::FIELDS);
const PAGE_OF_ATTEMPTS: Shape = Shape::Dto {
    fields: &page::FIELDS,
    nested: &[("items", list(&ATTEMPT))],
};

// Stacks.
const STACK: Shape = Shape::Dto {
    fields: &stack::FIELDS,
    nested: &[
        ("repositories", list(&dto(&stack_repository::FIELDS))),
        ("items", list(&dto(&stack_item::FIELDS))),
    ],
};

// Experiments.
const TRIAL: Shape = dto(&trial::FIELDS);
const STEP: Shape = Shape::Object(&[
    ("phase", Shape::Any),
    ("band", Shape::Any),
    ("reversals", Shape::Any),
    ("threshold", Shape::Any),
    ("visits", Shape::Any),
]);
const EXPERIMENT_REPORT_ARM: Shape = Shape::Object(&[
    ("arm", Shape::Any),
    ("trials", Shape::Any),
    ("bands", Shape::Any),
    ("fit", Shape::Any),
    ("fit_reason", Shape::Any),
    ("usage", Shape::Any),
    ("failure_causes", Shape::Any),
    ("invalid", Shape::Any),
]);

// Analytics.
const METRIC: Shape = dto(&metric_definition::FIELDS);
const WINDOW: Shape = dto(&agent_time_out::WINDOW_FIELDS);
const TOTALS: Shape = dto(&agent_time_out::TOTALS_FIELDS);

const ANALYTICS_REPORT: Shape = Shape::OneOf(&[
    // overview
    dto(&analytics_overview::FIELDS),
    // catalog
    Shape::Object(&[("metrics", list(&METRIC))]),
    // history
    Shape::Object(&[("tasks", list(&dto(&task_history_row::FIELDS)))]),
    // coupling
    Shape::Dto {
        fields: &coupling_out::FIELDS,
        nested: &[("summary", dto(&coupling_out::SUMMARY_FIELDS))],
    },
    // failures
    Shape::Dto {
        fields: &failures_out::FIELDS,
        nested: &[("findings", list(&dto(&failures_out::FINDING_FIELDS)))],
    },
    // estimate, enough history and not
    Shape::Dto {
        fields: &estimate_out::OK_FIELDS,
        nested: &[
            ("distributions", dto(&estimate_out::DISTRIBUTIONS_FIELDS)),
            ("verification", dto(&estimate_out::VERIFICATION_FIELDS)),
            ("basis", list(&dto(&estimate_out::BASIS_FIELDS))),
        ],
    },
    dto(&estimate_out::INSUFFICIENT_FIELDS),
]);

const TASK_IMPORT: Shape = Shape::Object(&[
    ("source", Shape::Any),
    ("dry_run", Shape::Any),
    (
        "tasks",
        list(&Shape::Object(&[
            ("id", Shape::Any),
            ("key", Shape::Any),
            ("title", Shape::Any),
            ("action", Shape::Any),
            ("status", Shape::Any),
            ("depends_on", Shape::Any),
            ("tags", Shape::Any),
        ])),
    ),
    ("map", Shape::Any),
    (
        "unresolved",
        list(&Shape::Object(&[
            ("id", Shape::Any),
            ("dependency", Shape::Any),
            ("reason", Shape::Any),
        ])),
    ),
    (
        "counts",
        Shape::Object(&[
            ("created", Shape::Any),
            ("updated", Shape::Any),
            ("unchanged", Shape::Any),
            ("ready", Shape::Any),
            ("backlog", Shape::Any),
        ]),
    ),
]);

const STARTED_REPOSITORY: Shape = Shape::Object(&[
    ("repository", Shape::Any),
    ("checkout_path", Shape::Any),
    ("branch", Shape::Any),
    ("base_sha", Shape::Any),
    ("base", Shape::Any),
]);

/// The keys of an `attempt_submit` answer for an attempt that did not succeed.
const SUBMIT_FAILED: Shape = Shape::Object(&[
    ("status", Shape::Any),
    ("error", Shape::Any),
    ("task_status", Shape::Any),
    ("attempt_status", Shape::Any),
]);
const SUBMIT_SUCCEEDED: Shape = Shape::Object(&[
    ("status", Shape::Any),
    ("verification", Shape::Any),
    ("outcome", Shape::Any),
    ("task_status", Shape::Any),
    ("attempt_status", Shape::Any),
]);

const EXPERIMENT_NEXT: Shape = Shape::Object(&[
    ("done", Shape::Any),
    ("phase", Shape::Any),
    ("trial", nullable(&TRIAL)),
    ("reason", Shape::Any),
    ("step", STEP),
]);

/// The output shape of every tool, by tool name.
static SHAPES: &[(&str, Shape)] = &[
    // M09-M16: stacks.
    ("stack_create", STACK),
    ("stack_get", STACK),
    ("stack_add_item", STACK),
    ("stack_remove_item", STACK),
    ("stack_list", Shape::Object(&[("stacks", list(&STACK))])),
    ("stack_history", Shape::Object(&[("events", list(&EVENT))])),
    (
        "task_get_history",
        Shape::Object(&[("events", list(&EVENT))]),
    ),
    (
        "stack_integrate",
        Shape::Object(&[
            ("stack", Shape::Any),
            (
                "jobs",
                list(&Shape::Object(&[
                    ("repository", Shape::Any),
                    ("job_id", Shape::Any),
                ])),
            ),
        ]),
    ),
    (
        "stack_verify",
        Shape::Object(&[("stack", Shape::Any), ("job_id", Shape::Any)]),
    ),
    // M17-M25, M28: experiments, trials, predictor, contract check.
    (
        "experiment_list",
        Shape::Object(&[("experiments", list(&dto(&experiment::FIELDS)))]),
    ),
    (
        "experiment_get",
        Shape::Object(&[
            ("experiment", dto(&experiment::FIELDS)),
            ("trials", list(&TRIAL)),
        ]),
    ),
    ("experiment_next", EXPERIMENT_NEXT),
    (
        "experiment_report",
        Shape::Object(&[
            ("experiment", Shape::Any),
            ("step", STEP),
            ("arms", list(&EXPERIMENT_REPORT_ARM)),
        ]),
    ),
    ("experiment_fit", dto(&predictor::FIT_FIELDS)),
    ("experiment_predict", dto(&predictor::PREDICTION_FIELDS)),
    (
        "trial_start",
        Shape::Object(&[
            ("trial", TRIAL),
            ("worktree", Shape::Any),
            ("branch", Shape::Any),
            ("base_sha", Shape::Any),
            ("brief", Shape::Any),
        ]),
    ),
    ("trial_grade", TRIAL),
    ("trial_classify", TRIAL),
    (
        "task_contract_check",
        Shape::Object(&[
            ("verdict", Shape::Any),
            ("reasons", Shape::Any),
            ("missing", Shape::Any),
            ("prediction", nullable(&dto(&predictor::PREDICTION_FIELDS))),
            ("target", Shape::Any),
            ("model", Shape::Any),
        ]),
    ),
    // M26-M46: tasks.
    ("task_create", TASK),
    ("task_update", TASK),
    ("task_claim", TASK),
    ("task_release", TASK),
    ("task_complete", TASK),
    ("task_report_result", TASK),
    ("task_add_dependency", TASK),
    ("task_remove_dependency", TASK),
    ("task_link_repository", TASK),
    (
        "task_get",
        Shape::Dto {
            fields: &task_detail::FIELDS,
            nested: &[
                ("files", list(&dto(&task_file::FIELDS))),
                ("relations", list(&dto(&relation::FIELDS))),
                ("comments", list(&COMMENT)),
                ("rollup", nullable(&dto(&task_rollup::FIELDS))),
                ("dependency_state", dto(&dependency_state::FIELDS)),
            ],
        },
    ),
    (
        "task_search",
        Shape::Dto {
            fields: &search_result::FIELDS,
            nested: &[("hits", list(&dto(&search_result::HIT_FIELDS)))],
        },
    ),
    ("task_list", PAGE_OF_TASKS),
    ("task_get_ready", Shape::Object(&[("tasks", list(&TASK))])),
    (
        "task_promote_ready",
        Shape::Object(&[
            ("promoted", Shape::Any),
            ("by_project", Shape::Any),
            ("dry_run", Shape::Any),
        ]),
    ),
    ("task_import_breakdown", TASK_IMPORT),
    ("task_add_relation", dto(&relation::FIELDS)),
    ("task_add_comment", COMMENT),
    ("task_report_progress", COMMENT),
    ("task_get_attempts", PAGE_OF_ATTEMPTS),
    ("attempt_list", PAGE_OF_ATTEMPTS),
    (
        "task_link_commit",
        Shape::Object(&[("links", list(&dto(&commit_link::FIELDS)))]),
    ),
    // M47-M54: attempts and runs.
    ("attempt_create", ATTEMPT),
    ("attempt_get", ATTEMPT),
    ("run_start", ATTEMPT),
    ("run_cancel", ATTEMPT),
    (
        "attempt_start",
        Shape::Object(&[
            ("attempt_id", Shape::Any),
            ("attempt_number", Shape::Any),
            ("task_key", Shape::Any),
            ("repositories", list(&STARTED_REPOSITORY)),
            ("agent_checks", Shape::Any),
        ]),
    ),
    (
        "attempt_submit",
        Shape::OneOf(&[SUBMIT_FAILED, SUBMIT_SUCCEEDED]),
    ),
    (
        "run_get",
        Shape::Object(&[
            ("attempt", ATTEMPT),
            ("job_id", Shape::Any),
            ("job_status", Shape::Any),
            ("recent_events", list(&EVENT)),
        ]),
    ),
    // M55-M56: git.
    (
        "git_get_changes",
        Shape::Dto {
            fields: &task_changes::FIELDS,
            nested: &[
                ("commits", list(&dto(&task_changes::COMMIT_FIELDS))),
                (
                    "final_change_surface",
                    list(&dto(&task_changes::SURFACE_FIELDS)),
                ),
            ],
        },
    ),
    ("git_get_task_diff", dto(&task_diff::FIELDS)),
    // M57-M63: classification, analytics.
    (
        "classification_get",
        Shape::Dto {
            fields: &classification_task::FIELDS,
            nested: &[
                ("current", nullable(&dto(&classification_out::FIELDS))),
                ("history", list(&dto(&classification_out::FIELDS))),
            ],
        },
    ),
    ("classification_request", dto(&classification_job::FIELDS)),
    (
        "analytics_query",
        Shape::Dto {
            fields: &analytics_query_out::FIELDS,
            nested: &[
                ("metric", METRIC),
                ("query", dto(&analytics_query::FIELDS)),
                ("provenance", dto(&analytics_query_out::PROVENANCE_FIELDS)),
            ],
        },
    ),
    ("analytics_report", ANALYTICS_REPORT),
    (
        "analytics_subagents",
        Shape::Dto {
            fields: &subagents_out::FIELDS,
            nested: &[("window", WINDOW)],
        },
    ),
    (
        "analytics_agent_time",
        Shape::Dto {
            fields: &agent_time_out::BREAKDOWN_FIELDS,
            nested: &[("window", WINDOW), ("totals", TOTALS)],
        },
    ),
    (
        "analytics_bottlenecks",
        Shape::Dto {
            fields: &agent_time_out::BOTTLENECKS_FIELDS,
            nested: &[
                ("window", WINDOW),
                ("totals", TOTALS),
                ("concurrency", dto(&agent_time_out::CONCURRENCY_FIELDS)),
            ],
        },
    ),
    // M64-M65 (automation) are the feature's shapes: `wg_automation_feature::spec::shapes`.
];

/// The output shape of a tool, by tool name. `None` for a name that is not a tool.
pub fn output_shape(tool: &str) -> Option<&'static Shape> {
    SHAPES
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, shape)| shape)
}

/// The names of the tools that have a shape, in table order.
pub fn tools_with_shape() -> impl Iterator<Item = &'static str> {
    SHAPES.iter().map(|(name, _)| *name)
}

/// `{"events": [...]}`: `stack_history` and `task_get_history`.
pub fn events(items: Vec<Value>) -> Value {
    dict(vec![("events", Value::List(items))])
}

/// `{"stacks": [...]}`: `stack_list`.
pub fn stacks(items: Vec<Value>) -> Value {
    dict(vec![("stacks", Value::List(items))])
}

/// `{"tasks": [...]}`: `task_get_ready`.
pub fn tasks(items: Vec<Value>) -> Value {
    dict(vec![("tasks", Value::List(items))])
}

/// `{"links": [...]}`: `task_link_commit`.
pub fn links(items: Vec<Value>) -> Value {
    dict(vec![("links", Value::List(items))])
}

/// `stack_integrate`: `{"stack": parent key, "jobs": [{"repository", "job_id"}, ...]}`.
pub fn stack_integrate(stack_key: &str, jobs: &[(&str, &str)]) -> Value {
    dict(vec![
        ("stack", text(stack_key)),
        (
            "jobs",
            Value::List(
                jobs.iter()
                    .map(|(repository, job_id)| {
                        dict(vec![
                            ("repository", text(repository)),
                            ("job_id", text(job_id)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// `stack_verify`: `{"stack": parent key, "job_id"}`.
pub fn stack_verify(stack_key: &str, job_id: &str) -> Value {
    dict(vec![("stack", text(stack_key)), ("job_id", text(job_id))])
}

/// `task_promote_ready`: the promoted keys, the count per project slug (`"(none)"` for a task in
/// no project) and whether it was a dry run.
pub fn promote_ready(promoted: &[&str], by_project: &[(String, i64)], dry_run: bool) -> Value {
    dict(vec![
        ("promoted", texts(promoted.iter().copied())),
        ("by_project", counts(by_project)),
        ("dry_run", Value::Bool(dry_run)),
    ])
}

/// One checkout of an `attempt_start` answer (`StartedRepository`).
pub fn started_repository(
    repository: &str,
    checkout_path: &str,
    branch: Option<&str>,
    base_sha: Option<&str>,
    base: Value,
) -> Value {
    dict(vec![
        ("repository", text(repository)),
        ("checkout_path", text(checkout_path)),
        ("branch", optional_text(branch)),
        ("base_sha", optional_text(base_sha)),
        ("base", base),
    ])
}

/// `attempt_start` (`AttemptStartOut`).
pub fn attempt_start(
    attempt_id: &str,
    attempt_number: i64,
    task_key: &str,
    repositories: Vec<Value>,
    agent_checks: Vec<Value>,
) -> Value {
    dict(vec![
        ("attempt_id", text(attempt_id)),
        ("attempt_number", int(attempt_number)),
        ("task_key", text(task_key)),
        ("repositories", Value::List(repositories)),
        ("agent_checks", Value::List(agent_checks)),
    ])
}

/// `attempt_submit` for an attempt that did not succeed: the conclusion's `status` and `error`,
/// then the task's and the attempt's status afterwards.
pub fn submit_failed(
    status: &str,
    error: Option<&str>,
    task_status: &str,
    attempt_status: &str,
) -> Value {
    dict(vec![
        ("status", text(status)),
        ("error", optional_text(error)),
        ("task_status", text(task_status)),
        ("attempt_status", text(attempt_status)),
    ])
}

/// `attempt_submit` for a succeeded attempt: `verification` is `null`, `{"gate"}`, `{"run_id",
/// "status"}` or `{"error"}`; `outcome` is the captured outcome without its commits.
pub fn submit_succeeded(
    verification: Value,
    outcome: Value,
    task_status: &str,
    attempt_status: &str,
) -> Value {
    dict(vec![
        ("status", text("succeeded")),
        ("verification", verification),
        ("outcome", outcome),
        ("task_status", text(task_status)),
        ("attempt_status", text(attempt_status)),
    ])
}

/// `run_get` (`RunOut`): the attempt, its `agent_run` job, and the recent events as
/// `Event.to_dict()` dicts (see [`super::event::dict_from_stored`]).
pub fn run_get(
    attempt: Value,
    job_id: Option<&str>,
    job_status: Option<&str>,
    recent_events: Vec<Value>,
) -> Value {
    dict(vec![
        ("attempt", attempt),
        ("job_id", optional_text(job_id)),
        ("job_status", optional_text(job_status)),
        ("recent_events", Value::List(recent_events)),
    ])
}

/// One planned task of a `task_import_breakdown` answer.
pub fn breakdown_row(
    id: &str,
    key: Option<&str>,
    title: &str,
    action: &str,
    status: &str,
    depends_on: &[&str],
    tags: &[&str],
) -> Value {
    dict(vec![
        ("id", text(id)),
        ("key", optional_text(key)),
        ("title", text(title)),
        ("action", text(action)),
        ("status", text(status)),
        ("depends_on", texts(depends_on.iter().copied())),
        ("tags", texts(tags.iter().copied())),
    ])
}

/// An unresolved dependency of a `task_import_breakdown` answer.
pub fn breakdown_unresolved(id: &str, dependency: &str, reason: &str) -> Value {
    dict(vec![
        ("id", text(id)),
        ("dependency", text(dependency)),
        ("reason", text(reason)),
    ])
}

/// `task_import_breakdown`: the plan. `map` is the section id to task key mapping in section
/// order (`null` for a task a dry run would create).
#[allow(clippy::too_many_arguments)]
pub fn import_breakdown(
    source: &str,
    dry_run: bool,
    rows: Vec<Value>,
    map: &[(&str, Option<&str>)],
    unresolved: Vec<Value>,
    created: i64,
    updated: i64,
    unchanged: i64,
    ready: i64,
    backlog: i64,
) -> Value {
    dict(vec![
        ("source", text(source)),
        ("dry_run", Value::Bool(dry_run)),
        ("tasks", Value::List(rows)),
        (
            "map",
            Value::Dict(
                map.iter()
                    .map(|(id, key)| (PyStr::from(*id), optional_text(*key)))
                    .collect(),
            ),
        ),
        ("unresolved", Value::List(unresolved)),
        (
            "counts",
            dict(vec![
                ("created", int(created)),
                ("updated", int(updated)),
                ("unchanged", int(unchanged)),
                ("ready", int(ready)),
                ("backlog", int(backlog)),
            ]),
        ),
    ])
}
