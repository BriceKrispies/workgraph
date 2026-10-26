//! The request models of the task commands (`TaskCreate`, `TaskUpdate`, `BulkUpdate`,
//! `TaskFilter`) and the typed values read from a validated request.
//!
//! A command collects its options into a JSON body, as the Python command builds the pydantic
//! model; [`validate_create`] and the others run the body through `wg-validate`, which gives the
//! `model_dump(mode="json")` value or the pydantic error list. The `read_*` functions then take
//! the validated value apart.

use serde_json::{Map, Value, json};
use wg_tasks::files::FileInput;
use wg_types::{ErrorCode, TaskPriority, TaskStatus, Timestamp, WgError};
use wg_validate::{
    Check, ChoiceKind, Extra, Field, IntRules, LoadOptions, Model, Schema, StrRules,
    ValidationError, model_from_json_schema, validate,
};

/// `handle_errors` on a pydantic `ValidationError`: `validation_failed` with the error list.
#[must_use]
pub fn validation_failure(errors: &[ValidationError]) -> WgError {
    let summary: Vec<String> = errors
        .iter()
        .map(|error| {
            let loc: Vec<String> = error
                .loc
                .iter()
                .map(|part| match part {
                    wg_validate::Loc::Key(key) => key.clone(),
                    wg_validate::Loc::Index(index) => index.to_string(),
                })
                .collect();
            let place = if loc.is_empty() {
                "input".to_owned()
            } else {
                loc.join(".")
            };
            format!("{place}: {}", error.msg)
        })
        .collect();
    WgError::new(
        ErrorCode::ValidationFailed,
        format!("invalid input: {}", summary.join("; ")),
    )
    .with_detail(
        "errors",
        Value::Array(errors.iter().map(ValidationError::to_json).collect()),
    )
}

fn check_for(model: &str, field: &str) -> Option<Check> {
    match (model, field) {
        ("TaskCreate", "tags") | ("TaskUpdate", "tags" | "add_tags" | "remove_tags") => {
            Some(Check::Tags)
        }
        ("TaskCreate", "files") | ("TaskUpdate", "files" | "add_files") => Some(Check::Files),
        ("TaskFileIn", "anchor") => Some(Check::EmptyToNone),
        _ => None,
    }
}

fn with_checks_schema(schema: Schema) -> Schema {
    match schema {
        Schema::Model(model) => Schema::Model(Box::new(with_checks(*model))),
        Schema::Nullable(inner) => Schema::Nullable(Box::new(with_checks_schema(*inner))),
        Schema::List {
            item,
            min_length,
            max_length,
        } => Schema::List {
            item: Box::new(with_checks_schema(*item)),
            min_length,
            max_length,
        },
        other => other,
    }
}

fn with_checks(model: Model) -> Model {
    let name = model.name.clone();
    Model {
        fields: model
            .fields
            .into_iter()
            .map(|field| Field {
                check: check_for(&name, &field.name),
                schema: with_checks_schema(field.schema),
                ..field
            })
            .collect(),
        ..model
    }
}

/// A request model from the OpenAPI component schema `name`, fields in declaration order.
pub(crate) fn component(name: &str, order: &[&str]) -> Result<Model, WgError> {
    let openapi = wg_wire::contract::openapi()?;
    let schema = &openapi["components"]["schemas"][name];
    let model = model_from_json_schema(
        &openapi,
        schema,
        LoadOptions {
            strip_whitespace: true,
        },
    )
    .map_err(|err| WgError::new(ErrorCode::WorkgraphError, format!("{name}: {err}")))?;
    Ok(with_checks(model.ordered(order)))
}

pub(crate) fn run(model: &Model, body: &Value) -> Result<Map<String, Value>, WgError> {
    match validate(model, body) {
        Ok(Value::Object(dump)) => Ok(dump),
        Ok(_) => Err(WgError::new(
            ErrorCode::WorkgraphError,
            "a request model did not give an object",
        )),
        Err(errors) => Err(validation_failure(&errors)),
    }
}

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

const UPDATE_ORDER: [&str; 22] = [
    "expected_version",
    "title",
    "description",
    "acceptance_criteria",
    "status",
    "priority",
    "tags",
    "add_tags",
    "remove_tags",
    "files",
    "add_files",
    "projects",
    "parent",
    "clear_parent",
    "assignee",
    "assigned_agent",
    "auto_execute",
    "due_at",
    "scheduled_at",
    "estimated_effort",
    "metadata",
    "reason",
];

/// Validates a `TaskCreate` body.
pub fn validate_create(body: &Value) -> Result<Map<String, Value>, WgError> {
    run(&component("TaskCreate", &CREATE_ORDER)?, body)
}

/// Validates a `TaskUpdate` body.
pub fn validate_update(body: &Value) -> Result<Map<String, Value>, WgError> {
    run(&component("TaskUpdate", &UPDATE_ORDER)?, body)
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

fn string_of(dump: &Map<String, Value>, key: &str) -> Option<String> {
    dump.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn strings_of(dump: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    dump.get(key).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()
    })
}

fn object_of(dump: &Map<String, Value>, key: &str) -> Option<Map<String, Value>> {
    dump.get(key).and_then(Value::as_object).cloned()
}

fn time_of(dump: &Map<String, Value>, key: &str) -> Result<Option<Timestamp>, WgError> {
    match dump.get(key).and_then(Value::as_str) {
        // A naive datetime counts as UTC (Python keeps it naive; the stored instant is the same).
        Some(text) => Timestamp::parse_iso(text)
            .or_else(|_| Timestamp::parse_iso(&format!("{text}Z")))
            .map(Some),
        None => Ok(None),
    }
}

fn files_of(dump: &Map<String, Value>, key: &str) -> Option<Vec<FileInput>> {
    dump.get(key).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .map(|item| FileInput {
                path: item["path"].as_str().unwrap_or_default().to_owned(),
                anchor: item["anchor"].as_str().map(str::to_owned),
                note: item["note"].as_str().unwrap_or_default().to_owned(),
            })
            .collect()
    })
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
        estimated_effort: object_of(dump, "estimated_effort"),
        metadata: object_of(dump, "metadata").unwrap_or_default(),
    })
}

/// A validated `TaskUpdate`, with references still as typed. A nullable field the request did
/// not name is `None`; one it named is `Some`, with `Some(None)` clearing it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateRead {
    pub expected_version: Option<i64>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance_criteria: Option<Vec<String>>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub tags: Option<Vec<String>>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub files: Option<Vec<FileInput>>,
    pub add_files: Vec<FileInput>,
    pub projects: Option<Vec<String>>,
    pub parent: Option<String>,
    pub clear_parent: bool,
    pub assignee: Option<Option<String>>,
    pub assigned_agent: Option<Option<String>>,
    pub auto_execute: Option<bool>,
    pub due_at: Option<Option<Timestamp>>,
    pub scheduled_at: Option<Option<Timestamp>>,
    pub estimated_effort: Option<Map<String, Value>>,
    pub metadata: Option<Map<String, Value>>,
    pub reason: Option<String>,
}

/// Reads a validated `TaskUpdate`; `given` is the request body, which tells a field that was
/// left out from one that was set to null (`model_dump(exclude_unset=True)`).
pub fn read_update(
    dump: &Map<String, Value>,
    given: &Map<String, Value>,
) -> Result<UpdateRead, WgError> {
    let named = |key: &str| given.contains_key(key);
    let clearable =
        |key: &str| -> Option<Option<String>> { named(key).then(|| string_of(dump, key)) };
    let clearable_time = |key: &str| -> Result<Option<Option<Timestamp>>, WgError> {
        if named(key) {
            time_of(dump, key).map(Some)
        } else {
            Ok(None)
        }
    };
    Ok(UpdateRead {
        expected_version: dump.get("expected_version").and_then(Value::as_i64),
        title: string_of(dump, "title"),
        description: string_of(dump, "description"),
        acceptance_criteria: strings_of(dump, "acceptance_criteria"),
        status: string_of(dump, "status").and_then(|text| TaskStatus::parse(&text)),
        priority: string_of(dump, "priority").and_then(|text| TaskPriority::parse(&text)),
        tags: strings_of(dump, "tags"),
        add_tags: strings_of(dump, "add_tags").unwrap_or_default(),
        remove_tags: strings_of(dump, "remove_tags").unwrap_or_default(),
        files: files_of(dump, "files"),
        add_files: files_of(dump, "add_files").unwrap_or_default(),
        projects: strings_of(dump, "projects"),
        parent: string_of(dump, "parent"),
        clear_parent: dump
            .get("clear_parent")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        assignee: clearable("assignee"),
        assigned_agent: clearable("assigned_agent"),
        auto_execute: dump.get("auto_execute").and_then(Value::as_bool),
        due_at: clearable_time("due_at")?,
        scheduled_at: clearable_time("scheduled_at")?,
        estimated_effort: object_of(dump, "estimated_effort"),
        metadata: object_of(dump, "metadata"),
        reason: string_of(dump, "reason"),
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
