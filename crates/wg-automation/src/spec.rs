//! The rule document: `RuleSpec` and its nested models (`automation/rules.py`), validated the way
//! Pydantic validates them and returned in the form the rule's columns store
//! (`model_dump(mode="json", exclude_none=True)`).
//!
//! The schema is data: each model is a list of [`Field`]s, and [`check_fields`] runs one of them.
//! Errors carry Pydantic's `type`, `loc`, `msg`, `input` and `ctx`, so the CLI and the API report
//! the details the Python service does. A model's own validator (the trigger's cross-field rules,
//! `create_dependency`'s one-of rule) runs only when that model's fields are valid, as Pydantic
//! runs it; the other models are checked whatever the others report.

use serde_json::{Map, Value, json};

use crate::condition::{self, Condition, ConditionError};
use crate::cron::Cron;
use crate::py;
use crate::rules::{TRIGGER_EVENTS, TRIGGER_TYPES};
use wg_types::pyval::{int_or, object, text, truthy};

/// The task priorities a rule may set (`TaskPriority`).
pub const PRIORITIES: [&str; 4] = ["critical", "high", "medium", "low"];

/// The statuses `update_task` may set.
pub const UPDATE_STATUSES: [&str; 4] = ["backlog", "ready", "blocked", "cancelled"];

/// The levels `notify` may use.
pub const LEVELS: [&str; 3] = ["info", "warning", "error"];

/// The action types in the order their union lists them (declaration order, which Pydantic uses
/// in the tags it reports).
const ACTION_ORDER: [&str; 9] = [
    "classify_task",
    "add_tags",
    "update_task",
    "create_child_tasks",
    "create_dependency",
    "enqueue_agent_run",
    "run_verification",
    "generate_report",
    "notify",
];

/// One validation error, as Pydantic reports it in `errors()`.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    /// Pydantic's error type, such as `missing` or `value_error`.
    pub kind: String,
    /// The path of the offending value: field names, union tags and list indices (numbers).
    pub loc: Vec<Value>,
    pub msg: String,
    pub input: Value,
    /// Pydantic's `ctx`, for the kinds that carry one.
    pub ctx: Option<Value>,
}

impl Issue {
    fn new(kind: &str, loc: &[Value], msg: impl Into<String>, input: &Value) -> Self {
        Issue {
            kind: kind.to_owned(),
            loc: loc.to_vec(),
            msg: msg.into(),
            input: input.clone(),
            ctx: None,
        }
    }

    fn with_ctx(self, ctx: Value) -> Self {
        Issue {
            ctx: Some(ctx),
            ..self
        }
    }

    /// The error as Pydantic's `errors(include_url=False)` lists it.
    pub fn to_json(&self) -> Value {
        let base = [
            ("type", json!(self.kind)),
            ("loc", Value::Array(self.loc.clone())),
            ("msg", json!(self.msg)),
            ("input", self.input.clone()),
        ];
        let ctx = self.ctx.as_ref().map(|ctx| ("ctx", ctx.clone()));
        Value::Object(
            base.into_iter()
                .chain(ctx)
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }
}

/// Why a rule document is refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// Pydantic's `ValidationError`: the errors, each with its location.
    Invalid(Vec<Issue>),
    /// A `ValidationFailed` the Python validators raise directly (a cron expression that does not
    /// parse): Pydantic does not wrap it, so the message is the error's.
    Raised(wg_types::WgError),
}

/// The kind of an issue that carries a raised error rather than a Pydantic error.
const RAISED: &str = "raised";

/// One field's outcome: its dumped entry (`None` when it is left out), or its errors.
type Checked = Result<Option<(String, Value)>, Vec<Issue>>;

/// A validated rule, in the column forms of `automation_rules`.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleSpec {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub trigger: Map<String, Value>,
    /// `{}` when the rule has no conditions.
    pub conditions: Map<String, Value>,
    pub actions: Vec<Map<String, Value>>,
    pub retry_max_attempts: i64,
    pub max_depth: i64,
}

impl RuleSpec {
    /// The trigger kind (`event`, `schedule` or `manual`).
    pub fn trigger_type(&self) -> &str {
        text(self.trigger.get("type"))
    }
}

/// The default of a field when the document leaves it out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dflt {
    /// No default: the field is required.
    Required,
    /// `None`: the field may be missing or null and is then left out of the dump.
    Omit,
    Str(&'static str),
    Bool(bool),
    Int(i64),
    Empty,
}

/// The type of a field.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// A string of `min..=max` characters (`max` open when `None`).
    Str {
        min: usize,
        max: Option<usize>,
    },
    Bool,
    /// An integer in `min..=max`.
    Int {
        min: i64,
        max: i64,
    },
    /// One of the listed strings.
    Lit(&'static [&'static str]),
    /// A list of `min..=max` items (`max` open when `None`).
    List {
        min: usize,
        max: Option<usize>,
        item: &'static Kind,
    },
    /// A nested model with no validator of its own.
    Model(&'static [Field]),
    /// The trigger model: its fields, then its cross-field rules.
    Trigger,
    /// A condition document (`Condition`).
    Condition,
    /// A list of actions, discriminated by `type`.
    Actions,
}

/// One field of a model.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub default: Dflt,
}

const fn field(name: &'static str, kind: Kind, default: Dflt) -> Field {
    Field {
        name,
        kind,
        default,
    }
}

const STR_ANY: Kind = Kind::Str { min: 0, max: None };
const STR_NAME: Kind = Kind::Str {
    min: 1,
    max: Some(200),
};
const STR_TITLE: Kind = Kind::Str {
    min: 1,
    max: Some(300),
};
const STR_BODY: Kind = Kind::Str {
    min: 0,
    max: Some(5000),
};
const TRIGGER_KINDS: Kind = Kind::Lit(&TRIGGER_TYPES);
const PRIORITY: Kind = Kind::Lit(&PRIORITIES);
const UPDATE_STATUS: Kind = Kind::Lit(&UPDATE_STATUSES);
const LEVEL: Kind = Kind::Lit(&LEVELS);
const TAG_ITEM: Kind = Kind::Str { min: 0, max: None };
const EVENT_TYPES: Kind = Kind::List {
    min: 0,
    max: None,
    item: &TAG_ITEM,
};
const TAGS: Kind = Kind::List {
    min: 0,
    max: None,
    item: &TAG_ITEM,
};
const ADD_TAGS: Kind = Kind::List {
    min: 1,
    max: Some(20),
    item: &TAG_ITEM,
};
const CHILD_FIELDS: &[Field] = &[
    field("title", STR_TITLE, Dflt::Required),
    field(
        "description",
        Kind::Str {
            min: 0,
            max: Some(5000),
        },
        Dflt::Str(""),
    ),
    field("priority", PRIORITY, Dflt::Omit),
    field("tags", TAGS, Dflt::Empty),
];
const CHILD: Kind = Kind::Model(CHILD_FIELDS);
const CHILDREN: Kind = Kind::List {
    min: 1,
    max: Some(20),
    item: &CHILD,
};

const TRIGGER_FIELDS: &[Field] = &[
    field("type", TRIGGER_KINDS, Dflt::Required),
    field("event_types", EVENT_TYPES, Dflt::Empty),
    field("cron", STR_ANY, Dflt::Omit),
    field(
        "interval_seconds",
        Kind::Int {
            min: 60,
            max: i64::MAX,
        },
        Dflt::Omit,
    ),
];

const RULE_FIELDS: &[Field] = &[
    field("name", STR_NAME, Dflt::Required),
    field("description", STR_ANY, Dflt::Str("")),
    field("enabled", Kind::Bool, Dflt::Bool(true)),
    field("trigger", Kind::Trigger, Dflt::Required),
    field("conditions", Kind::Condition, Dflt::Omit),
    field("actions", Kind::Actions, Dflt::Required),
    field(
        "retry_max_attempts",
        Kind::Int { min: 1, max: 10 },
        Dflt::Int(3),
    ),
    field("max_depth", Kind::Int { min: 1, max: 10 }, Dflt::Int(3)),
];

const NO_FIELDS: &[Field] = &[];
const ADD_TAGS_FIELDS: &[Field] = &[field("tags", ADD_TAGS, Dflt::Required)];
const UPDATE_TASK_FIELDS: &[Field] = &[
    field("priority", PRIORITY, Dflt::Omit),
    field("status", UPDATE_STATUS, Dflt::Omit),
    field("assignee", STR_ANY, Dflt::Omit),
    field("assigned_agent", STR_ANY, Dflt::Omit),
    field("auto_execute", Kind::Bool, Dflt::Omit),
];
const CREATE_CHILD_TASKS_FIELDS: &[Field] = &[
    field("children", CHILDREN, Dflt::Required),
    field("sequential", Kind::Bool, Dflt::Bool(false)),
    field("inherit_repositories", Kind::Bool, Dflt::Bool(true)),
];
const CREATE_DEPENDENCY_FIELDS: &[Field] = &[
    field("depends_on", STR_ANY, Dflt::Omit),
    field("blocks", STR_ANY, Dflt::Omit),
];
const ENQUEUE_AGENT_RUN_FIELDS: &[Field] = &[
    field("agent", STR_ANY, Dflt::Omit),
    field("model", STR_ANY, Dflt::Omit),
];
const GENERATE_REPORT_FIELDS: &[Field] = &[field(
    "period_days",
    Kind::Int { min: 1, max: 366 },
    Dflt::Int(7),
)];
const NOTIFY_FIELDS: &[Field] = &[
    field("level", LEVEL, Dflt::Str("info")),
    field("title", STR_TITLE, Dflt::Required),
    field("body", STR_BODY, Dflt::Str("")),
];

/// The fields of one action type, in the order its model declares them (`type` comes first).
fn action_fields(kind: &str) -> Option<&'static [Field]> {
    match kind {
        "classify_task" | "run_verification" => Some(NO_FIELDS),
        "add_tags" => Some(ADD_TAGS_FIELDS),
        "update_task" => Some(UPDATE_TASK_FIELDS),
        "create_child_tasks" => Some(CREATE_CHILD_TASKS_FIELDS),
        "create_dependency" => Some(CREATE_DEPENDENCY_FIELDS),
        "enqueue_agent_run" => Some(ENQUEUE_AGENT_RUN_FIELDS),
        "generate_report" => Some(GENERATE_REPORT_FIELDS),
        "notify" => Some(NOTIFY_FIELDS),
        _ => None,
    }
}

/// The Pydantic type error for a value of the wrong JSON type.
fn type_error(kind: &Kind, input: &Value, loc: &[Value]) -> Issue {
    match kind {
        Kind::Str { .. } => Issue::new("string_type", loc, "Input should be a valid string", input),
        Kind::Bool => Issue::new(
            "bool_type",
            loc,
            "Input should be a valid boolean, unable to interpret input",
            input,
        ),
        Kind::Int { .. } => Issue::new("int_type", loc, "Input should be a valid integer", input),
        Kind::Lit(allowed) => Issue::new(
            "literal_error",
            loc,
            format!("Input should be {}", or_list(allowed)),
            input,
        )
        .with_ctx(json!({ "expected": or_list(allowed) })),
        Kind::List { .. } | Kind::Actions => {
            Issue::new("list_type", loc, "Input should be a valid list", input)
        }
        Kind::Model(_) | Kind::Trigger => Issue::new(
            "model_type",
            loc,
            "Input should be a valid dictionary or object to extract fields from",
            input,
        ),
        Kind::Condition => Issue::new(
            "dict_type",
            loc,
            "Input should be a valid dictionary",
            input,
        ),
    }
}

/// `'a', 'b' or 'c'`: Pydantic's wording for a literal's choices.
fn or_list(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| format!("'{item}'")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// `loc` with one more part (a field name, a union tag or a list index).
fn child_loc(loc: &[Value], part: Value) -> Vec<Value> {
    loc.iter().cloned().chain(std::iter::once(part)).collect()
}

/// An integer as Pydantic's lax mode reads one: JSON integers, and floats with no fraction.
fn whole_number(input: &Value) -> Option<i64> {
    match input {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|float| float.fract() == 0.0)
                .map(|float| float as i64)
        }),
        _ => None,
    }
}

/// Splits a list of results into the values and the issues of the failed ones.
fn partition<T>(results: Vec<Result<T, Vec<Issue>>>) -> (Vec<T>, Vec<Issue>) {
    let issues: Vec<Issue> = results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .flatten()
        .cloned()
        .collect();
    let values: Vec<T> = results.into_iter().filter_map(Result::ok).collect();
    (values, issues)
}

/// Validates one value against a kind, returning its dumped form.
fn check_value(kind: &Kind, input: &Value, loc: &[Value]) -> Result<Value, Vec<Issue>> {
    match kind {
        Kind::Str { min, max } => match input.as_str() {
            Some(text) => {
                let length = text.chars().count();
                if length < *min {
                    Err(vec![
                        Issue::new(
                            "string_too_short",
                            loc,
                            format!(
                                "String should have at least {min} character{}",
                                plural(*min)
                            ),
                            input,
                        )
                        .with_ctx(json!({ "min_length": min })),
                    ])
                } else if max.is_some_and(|max| length > max) {
                    let limit = max.unwrap_or(0);
                    Err(vec![
                        Issue::new(
                            "string_too_long",
                            loc,
                            format!("String should have at most {limit} characters"),
                            input,
                        )
                        .with_ctx(json!({ "max_length": limit })),
                    ])
                } else {
                    Ok(input.clone())
                }
            }
            None => Err(vec![type_error(kind, input, loc)]),
        },
        Kind::Bool => match input.as_bool() {
            Some(_) => Ok(input.clone()),
            None => Err(vec![type_error(kind, input, loc)]),
        },
        Kind::Int { min, max } => match whole_number(input) {
            Some(number) if number < *min => Err(vec![
                Issue::new(
                    "greater_than_equal",
                    loc,
                    format!("Input should be greater than or equal to {min}"),
                    input,
                )
                .with_ctx(json!({ "ge": min })),
            ]),
            Some(number) if number > *max => Err(vec![
                Issue::new(
                    "less_than_equal",
                    loc,
                    format!("Input should be less than or equal to {max}"),
                    input,
                )
                .with_ctx(json!({ "le": max })),
            ]),
            Some(number) => Ok(json!(number)),
            None => Err(vec![type_error(kind, input, loc)]),
        },
        Kind::Lit(allowed) => match input.as_str() {
            Some(text) if allowed.contains(&text) => Ok(input.clone()),
            _ => Err(vec![type_error(kind, input, loc)]),
        },
        Kind::List { min, max, item } => match input.as_array() {
            Some(items) => {
                let count = items.len();
                if count < *min {
                    Err(vec![Issue::new(
                        "too_short",
                        loc,
                        format!(
                            "List should have at least {min} item{} after validation, not {count}",
                            plural(*min)
                        ),
                        input,
                    )
                    .with_ctx(json!({
                        "field_type": "List",
                        "min_length": min,
                        "actual_length": count,
                    }))])
                } else if max.is_some_and(|max| count > max) {
                    let limit = max.unwrap_or(0);
                    Err(vec![Issue::new(
                        "too_long",
                        loc,
                        format!("List should have at most {limit} items after validation, not {count}"),
                        input,
                    )
                    .with_ctx(json!({
                        "field_type": "List",
                        "max_length": limit,
                        "actual_length": count,
                    }))])
                } else {
                    let checked = items
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            check_value(item, value, &child_loc(loc, json!(index)))
                        })
                        .collect();
                    let (values, issues) = partition(checked);
                    if issues.is_empty() {
                        Ok(Value::Array(values))
                    } else {
                        Err(issues)
                    }
                }
            }
            None => Err(vec![type_error(kind, input, loc)]),
        },
        Kind::Model(fields) => match input.as_object() {
            Some(map) => check_fields(map, loc, fields).map(Value::Object),
            None => Err(vec![type_error(kind, input, loc)]),
        },
        Kind::Trigger => match input.as_object() {
            Some(map) => {
                let dumped = check_fields(map, loc, TRIGGER_FIELDS)?;
                trigger_rules(map, loc)?;
                Ok(Value::Object(dumped))
            }
            None => Err(vec![type_error(kind, input, loc)]),
        },
        Kind::Condition => match condition::validate(input) {
            Ok(parsed) => Ok(Value::Object(condition_dump(&parsed))),
            Err(ConditionError::Invalid(msg) | ConditionError::BadPattern(msg)) => {
                Err(vec![value_error(loc, &msg, input)])
            }
        },
        Kind::Actions => check_actions(input, loc).map(Value::Array),
    }
}

/// A Pydantic `value_error`: a model validator's message, with its `ctx.error`.
fn value_error(loc: &[Value], msg: &str, input: &Value) -> Issue {
    Issue::new("value_error", loc, format!("Value error, {msg}"), input)
        .with_ctx(json!({ "error": msg }))
}

/// Checks a model's fields and rejects extra keys. Returns the dump: the fields that are present,
/// in declaration order.
fn check_fields(
    map: &Map<String, Value>,
    loc: &[Value],
    fields: &[Field],
) -> Result<Map<String, Value>, Vec<Issue>> {
    let extras: Vec<Checked> = map
        .iter()
        .filter(|(key, _)| !fields.iter().any(|f| f.name == key.as_str()))
        .map(|(key, value)| {
            Err(vec![Issue::new(
                "extra_forbidden",
                &child_loc(loc, json!(key)),
                "Extra inputs are not permitted",
                value,
            )])
        })
        .collect();
    let checked: Vec<Checked> = fields.iter().map(|f| check_field(map, loc, f)).collect();
    let (dumped, issues) = partition(checked.into_iter().chain(extras).collect());
    if issues.is_empty() {
        Ok(dumped.into_iter().flatten().collect())
    } else {
        Err(issues)
    }
}

/// One field of a model: its dumped entry, or `None` when it is left out.
fn check_field(
    map: &Map<String, Value>,
    loc: &[Value],
    field: &Field,
) -> Result<Option<(String, Value)>, Vec<Issue>> {
    let field_loc = child_loc(loc, json!(field.name));
    let entry = |value: Value| Ok(Some((field.name.to_owned(), value)));
    match map.get(field.name) {
        None => match field.default {
            Dflt::Required => Err(vec![Issue::new(
                "missing",
                &field_loc,
                "Field required",
                &Value::Object(map.clone()),
            )]),
            Dflt::Omit => Ok(None),
            Dflt::Str(text) => entry(json!(text)),
            Dflt::Bool(flag) => entry(json!(flag)),
            Dflt::Int(number) => entry(json!(number)),
            Dflt::Empty => entry(json!([])),
        },
        Some(Value::Null) if field.default == Dflt::Omit => Ok(None),
        Some(value) => check_value(&field.kind, value, &field_loc).and_then(entry),
    }
}

/// `Condition`'s dump: `all`, `any`, `not` and `predicate`, the present ones.
pub fn condition_dump(condition: &Condition) -> Map<String, Value> {
    let list = |items: &Vec<Condition>| {
        Value::Array(
            items
                .iter()
                .map(|item| Value::Object(condition_dump(item)))
                .collect(),
        )
    };
    let all = condition
        .all
        .as_ref()
        .map(|items| ("all".to_owned(), list(items)));
    let any = condition
        .any
        .as_ref()
        .map(|items| ("any".to_owned(), list(items)));
    let not = condition
        .not
        .as_ref()
        .map(|inner| ("not".to_owned(), Value::Object(condition_dump(inner))));
    let predicate = condition.predicate.as_ref().map(|predicate| {
        let value =
            (!predicate.value.is_null()).then(|| ("value".to_owned(), predicate.value.clone()));
        let entry: Map<String, Value> = [
            ("field".to_owned(), json!(predicate.field)),
            ("op".to_owned(), json!(predicate.op.name())),
        ]
        .into_iter()
        .chain(value)
        .collect();
        ("predicate".to_owned(), Value::Object(entry))
    });
    [all, any, not, predicate].into_iter().flatten().collect()
}

/// The actions list: each item checked against the model its `type` names.
fn check_actions(input: &Value, loc: &[Value]) -> Result<Vec<Value>, Vec<Issue>> {
    let Some(items) = input.as_array() else {
        return Err(vec![type_error(&Kind::Actions, input, loc)]);
    };
    let count = items.len();
    if count < 1 {
        return Err(vec![
            Issue::new(
                "too_short",
                loc,
                "List should have at least 1 item after validation, not 0",
                input,
            )
            .with_ctx(json!({ "field_type": "List", "min_length": 1, "actual_length": 0 })),
        ]);
    }
    if count > 20 {
        return Err(vec![
            Issue::new(
                "too_long",
                loc,
                format!("List should have at most 20 items after validation, not {count}"),
                input,
            )
            .with_ctx(json!({ "field_type": "List", "max_length": 20, "actual_length": count })),
        ]);
    }
    let checked = items
        .iter()
        .enumerate()
        .map(|(index, item)| check_action(item, &child_loc(loc, json!(index))))
        .collect();
    let (values, issues) = partition(checked);
    if issues.is_empty() {
        Ok(values)
    } else {
        Err(issues)
    }
}

/// One action: its `type` picks the model, the model's fields are checked, and the model's
/// validator runs on the valid fields. The tag is part of the location, as Pydantic's
/// discriminated unions report it.
fn check_action(item: &Value, loc: &[Value]) -> Result<Value, Vec<Issue>> {
    let discriminator = || {
        Err(vec![
            Issue::new(
                "union_tag_not_found",
                loc,
                "Unable to extract tag using discriminator 'type'",
                item,
            )
            .with_ctx(json!({ "discriminator": "'type'" })),
        ])
    };
    let Some(map) = item.as_object() else {
        return discriminator();
    };
    let Some(tag) = map.get("type").and_then(Value::as_str) else {
        return discriminator();
    };
    let Some(fields) = action_fields(tag) else {
        let expected: Vec<String> = ACTION_ORDER
            .iter()
            .map(|name| format!("'{name}'"))
            .collect();
        let expected_text = expected.join(", ");
        return Err(vec![Issue::new(
            "union_tag_invalid",
            loc,
            format!(
                "Input tag '{tag}' found using 'type' does not match any of the expected tags: {expected_text}"
            ),
            item,
        )
        .with_ctx(json!({
            "discriminator": "'type'",
            "tag": tag,
            "expected_tags": expected_text,
        }))]);
    };
    let tag_loc = child_loc(loc, json!(tag));
    // The `type` key is the discriminator, not a field of the model.
    let body: Map<String, Value> = map
        .iter()
        .filter(|(key, _)| key.as_str() != "type")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let dumped = check_fields(&body, &tag_loc, fields)?;
    action_rules(tag, &dumped, map, &tag_loc)?;
    let entry: Map<String, Value> = std::iter::once(("type".to_owned(), json!(tag)))
        .chain(dumped)
        .collect();
    Ok(Value::Object(entry))
}

/// The action models' validators: `create_dependency` takes exactly one of its two targets. The
/// error's input is the action as given.
fn action_rules(
    tag: &str,
    dumped: &Map<String, Value>,
    given: &Map<String, Value>,
    loc: &[Value],
) -> Result<(), Vec<Issue>> {
    let set = |name: &str| dumped.get(name).is_some_and(truthy);
    if tag == "create_dependency" && set("depends_on") == set("blocks") {
        return Err(vec![value_error(
            loc,
            "specify exactly one of depends_on or blocks",
            &Value::Object(given.clone()),
        )]);
    }
    Ok(())
}

/// The trigger's validator: event types known, schedules with exactly one of cron or interval, and
/// a cron expression that parses. The error's input is the trigger as given.
fn trigger_rules(trigger: &Map<String, Value>, loc: &[Value]) -> Result<(), Vec<Issue>> {
    let input = Value::Object(trigger.clone());
    let fail = |msg: String| Err(vec![value_error(loc, &msg, &input)]);
    match text(trigger.get("type")) {
        "event" => event_trigger_rules(trigger).or_else(fail),
        "schedule" => {
            let cron = trigger.get("cron").filter(|value| truthy(value));
            let interval = trigger
                .get("interval_seconds")
                .filter(|value| truthy(value));
            if cron.is_some() == interval.is_some() {
                return fail(
                    "schedule triggers need exactly one of cron or interval_seconds".to_owned(),
                );
            }
            match cron.and_then(Value::as_str) {
                Some(expression) => Cron::parse(expression)
                    .map(|_| ())
                    .map_err(|error| vec![Issue::new(RAISED, &[], error.message, &Value::Null)]),
                None => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

/// The event trigger's rule: at least one event type, all of them known.
fn event_trigger_rules(trigger: &Map<String, Value>) -> Result<(), String> {
    let types: Vec<Value> = trigger
        .get("event_types")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if types.is_empty() {
        return Err("event triggers need at least one event type".to_owned());
    }
    let allowed = |name: &str| TRIGGER_EVENTS.iter().any(|(event, _)| *event == name);
    let unknown: Vec<Value> = types
        .iter()
        .filter(|name| !name.as_str().is_some_and(allowed))
        .cloned()
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    let names: std::collections::BTreeSet<String> = TRIGGER_EVENTS
        .iter()
        .map(|(event, _)| (*event).to_owned())
        .collect();
    let allowed_list = Value::Array(names.into_iter().map(Value::String).collect());
    Err(format!(
        "unknown trigger events {}; allowed: {}",
        py::repr(&Value::Array(unknown)),
        py::repr(&allowed_list)
    ))
}

/// Validates a rule document (`RuleSpec.model_validate`).
pub fn validate_rule(value: &Value) -> Result<RuleSpec, Failure> {
    let root: Vec<Value> = Vec::new();
    let Some(map) = value.as_object() else {
        return Err(Failure::Invalid(vec![type_error(
            &Kind::Model(RULE_FIELDS),
            value,
            &root,
        )]));
    };
    let dumped = check_fields(map, &root, RULE_FIELDS).map_err(|issues| {
        match issues.iter().find(|issue| issue.kind == RAISED) {
            Some(raised) => Failure::Raised(wg_types::WgError::validation(raised.msg.clone())),
            None => Failure::Invalid(issues),
        }
    })?;
    let trigger = object(dumped.get("trigger"));
    Ok(RuleSpec {
        name: text(dumped.get("name")).to_owned(),
        description: text(dumped.get("description")).to_owned(),
        enabled: dumped
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        trigger,
        conditions: object(dumped.get("conditions")),
        actions: dumped
            .get("actions")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_object().cloned())
                    .collect()
            })
            .unwrap_or_default(),
        retry_max_attempts: int_or(dumped.get("retry_max_attempts"), 3),
        max_depth: int_or(dumped.get("max_depth"), 3),
    })
}
