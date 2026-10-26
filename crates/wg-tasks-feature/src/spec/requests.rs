//! The request models of the task commands (`TaskCreate`, `TaskUpdate`, `BulkUpdate`,
//! `TaskFilter`) and the typed values read from a validated request.
//!
//! A command collects its options into a JSON body, as the Python command builds the pydantic
//! model; [`validate_create`] and the others run the body through `wg-validate`, which gives the
//! `model_dump(mode="json")` value or the pydantic error list. The `read_*` functions then take
//! the validated value apart.

use serde_json::{Map, Value, json};
use wg_tasks::edit::*;
use wg_tasks::files::FileInput;
use wg_tasks::requests::{component, run};
use wg_types::{TaskPriority, TaskStatus, Timestamp, WgError};
use wg_validate::{ChoiceKind, Extra, Field, IntRules, Model, Schema, StrRules};

const CREATE_ORDER: [&str; 19] = [
    "key",
    "title",
    "description",
    "acceptance_criteria",
    "status",
    "priority",
    "tags",
    "files",
    "projects",
    "repositories",
    "parent",
    "depends_on",
    "assignee",
    "assigned_agent",
    "auto_execute",
    "due_at",
    "scheduled_at",
    "estimated_effort",
    "metadata",
];

/// Validates a `TaskCreate` body.
pub fn validate_create(body: &Value) -> Result<Map<String, Value>, WgError> {
    run(&component("TaskCreate", &CREATE_ORDER)?, body)
}

/// Validates a `BulkUpdate` body (`tasks` and the `update` it applies).
pub fn validate_bulk(body: &Value) -> Result<Map<String, Value>, WgError> {
    let update = component("TaskUpdate", &UPDATE_ORDER)?;
    let bulk = component("BulkUpdate", &["tasks", "update"])?;
    let bulk = bulk.map_field("update", |field| Field {
        schema: Schema::Model(Box::new(update.clone())),
        ..field
    });
    run(&bulk, body)
}

fn enum_of(openapi: &Value, name: &str) -> Schema {
    let values = openapi["components"]["schemas"][name]["enum"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Schema::Choice {
        kind: ChoiceKind::Enum,
        values,
    }
}

fn text() -> Schema {
    Schema::Str(StrRules {
        strip: true,
        ..StrRules::default()
    })
}

fn maybe_text(name: &str) -> Field {
    Field::optional(name, Schema::Nullable(Box::new(text())), Value::Null)
}

fn maybe_list(name: &str, item: Schema) -> Field {
    Field::optional(
        name,
        Schema::Nullable(Box::new(Schema::List {
            item: Box::new(item),
            min_length: None,
            max_length: None,
        })),
        Value::Null,
    )
}

fn maybe_time(name: &str) -> Field {
    Field::optional(
        name,
        Schema::Nullable(Box::new(Schema::DateTime)),
        Value::Null,
    )
}

fn flag(name: &str) -> Field {
    Field::optional(name, Schema::Bool, json!(false))
}

/// `TaskFilter`, which the OpenAPI document does not carry.
fn filter_model() -> Result<Model, WgError> {
    let openapi = wg_wire::contract::openapi()?;
    let sort = Schema::Choice {
        kind: ChoiceKind::Literal,
        values: [
            "created_at",
            "updated_at",
            "priority",
            "key",
            "title",
            "status",
            "due_at",
        ]
        .map(str::to_owned)
        .to_vec(),
    };
    Ok(Model::new(
        "TaskFilter",
        Extra::Forbid,
        vec![
            maybe_list("statuses", enum_of(&openapi, "TaskStatus")),
            maybe_list("priorities", enum_of(&openapi, "TaskPriority")),
            maybe_text("project"),
            maybe_text("repository"),
            maybe_list("tags", text()),
            maybe_text("file"),
            maybe_text("category"),
            maybe_text("assignee"),
            maybe_text("parent"),
            flag("root_only"),
            maybe_text("text"),
            maybe_time("created_after"),
            maybe_time("created_before"),
            flag("include_archived"),
            flag("archived_only"),
            Field::optional("sort", sort, json!("created_at")),
            Field::optional("descending", Schema::Bool, json!(true)),
            Field::optional(
                "limit",
                Schema::Int(IntRules {
                    ge: Some(1),
                    le: Some(500),
                    ..IntRules::default()
                }),
                json!(50),
            ),
            Field::optional(
                "offset",
                Schema::Int(IntRules {
                    ge: Some(0),
                    ..IntRules::default()
                }),
                json!(0),
            ),
        ],
    ))
}

/// Validates a `TaskFilter` body.
pub fn validate_filter(body: &Value) -> Result<Map<String, Value>, WgError> {
    run(&filter_model()?, body)
}

/// A validated `TaskCreate`, with references still as typed.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateRead {
    pub key: Option<String>,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub tags: Vec<String>,
    pub files: Vec<FileInput>,
    pub projects: Vec<String>,
    pub repositories: Vec<String>,
    pub parent: Option<String>,
    pub depends_on: Vec<String>,
    pub assignee: Option<String>,
    pub assigned_agent: Option<String>,
    pub auto_execute: bool,
    pub due_at: Option<Timestamp>,
    pub scheduled_at: Option<Timestamp>,
    /// `due_at` was given without an offset (a naive datetime).
    pub due_naive: bool,
    /// `scheduled_at` was given without an offset (a naive datetime).
    pub scheduled_naive: bool,
    pub estimated_effort: Option<Map<String, Value>>,
    pub metadata: Map<String, Value>,
}

/// Reads a validated `TaskCreate`.
pub fn read_create(dump: &Map<String, Value>) -> Result<CreateRead, WgError> {
    let status = string_of(dump, "status")
        .and_then(|text| TaskStatus::parse(&text))
        .unwrap_or(TaskStatus::Backlog);
    let priority = string_of(dump, "priority")
        .and_then(|text| TaskPriority::parse(&text))
        .unwrap_or(TaskPriority::Medium);
    Ok(CreateRead {
        key: string_of(dump, "key"),
        title: string_of(dump, "title").unwrap_or_default(),
        description: string_of(dump, "description").unwrap_or_default(),
        acceptance_criteria: strings_of(dump, "acceptance_criteria").unwrap_or_default(),
        status,
        priority,
        tags: strings_of(dump, "tags").unwrap_or_default(),
        files: files_of(dump, "files").unwrap_or_default(),
        projects: strings_of(dump, "projects").unwrap_or_default(),
        repositories: strings_of(dump, "repositories").unwrap_or_default(),
        parent: string_of(dump, "parent"),
        depends_on: strings_of(dump, "depends_on").unwrap_or_default(),
        assignee: string_of(dump, "assignee"),
        assigned_agent: string_of(dump, "assigned_agent"),
        auto_execute: dump
            .get("auto_execute")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        due_at: time_of(dump, "due_at")?,
        scheduled_at: time_of(dump, "scheduled_at")?,
        due_naive: is_naive(dump, "due_at"),
        scheduled_naive: is_naive(dump, "scheduled_at"),
        estimated_effort: object_of(dump, "estimated_effort"),
        metadata: object_of(dump, "metadata").unwrap_or_default(),
    })
}

/// A validated `TaskFilter`.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterRead {
    pub statuses: Vec<TaskStatus>,
    pub priorities: Vec<TaskPriority>,
    pub project: Option<String>,
    pub repository: Option<String>,
    pub tags: Vec<String>,
    pub file: Option<String>,
    pub category: Option<String>,
    pub assignee: Option<String>,
    pub parent: Option<String>,
    pub root_only: bool,
    pub text: Option<String>,
    pub created_after: Option<Timestamp>,
    pub created_before: Option<Timestamp>,
    pub include_archived: bool,
    pub archived_only: bool,
    pub sort: String,
    pub descending: bool,
    pub limit: i64,
    pub offset: i64,
}

/// Reads a validated `TaskFilter`. An empty string counts as unset, as a falsy filter does in
/// Python.
pub fn read_filter(dump: &Map<String, Value>) -> Result<FilterRead, WgError> {
    let non_empty = |key: &str| string_of(dump, key).filter(|text| !text.is_empty());
    let flag = |key: &str| dump.get(key).and_then(Value::as_bool).unwrap_or(false);
    Ok(FilterRead {
        statuses: strings_of(dump, "statuses")
            .unwrap_or_default()
            .iter()
            .filter_map(|text| TaskStatus::parse(text))
            .collect(),
        priorities: strings_of(dump, "priorities")
            .unwrap_or_default()
            .iter()
            .filter_map(|text| TaskPriority::parse(text))
            .collect(),
        project: non_empty("project"),
        repository: non_empty("repository"),
        tags: strings_of(dump, "tags").unwrap_or_default(),
        file: non_empty("file"),
        category: non_empty("category"),
        assignee: non_empty("assignee"),
        parent: non_empty("parent"),
        root_only: flag("root_only"),
        text: non_empty("text"),
        created_after: time_of(dump, "created_after")?,
        created_before: time_of(dump, "created_before")?,
        include_archived: flag("include_archived"),
        archived_only: flag("archived_only"),
        sort: string_of(dump, "sort").unwrap_or_else(|| "created_at".to_owned()),
        descending: dump
            .get("descending")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        limit: dump.get("limit").and_then(Value::as_i64).unwrap_or(50),
        offset: dump.get("offset").and_then(Value::as_i64).unwrap_or(0),
    })
}
