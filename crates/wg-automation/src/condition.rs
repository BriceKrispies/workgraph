//! Declarative rule conditions and their deterministic evaluator (`automation/rules.py`).
//!
//! A condition is `{"all": [...]}`, `{"any": [...]}`, `{"not": {...}}`, `{"predicate": {...}}`
//! or a bare predicate `{"field": ..., "op": ..., "value": ...}`. [`validate`] reproduces the
//! Pydantic model: unknown keys, bad operators and oversized values are `Invalid`; a `matches`
//! pattern that does not compile is `BadPattern`, which Python raises as `re.error` and which
//! wins over `Invalid` because it escapes validation unwrapped.

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::py::{compare, contains, eq, truthy};

/// Longest string value a predicate may compare with, in characters.
pub const MAX_VALUE_CHARS: usize = 500;

/// Longest prefix of the actual value a `matches` predicate searches.
pub const MATCH_PREFIX_CHARS: usize = 10_000;

/// The comparison operators, in the order of the Python `Operator` literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Eq,
    Ne,
    In,
    NotIn,
    Contains,
    NotContains,
    Gt,
    Gte,
    Lt,
    Lte,
    Exists,
    NotExists,
    Matches,
    StartsWith,
}

/// Every operator, in declaration order.
pub const OPERATORS: [Operator; 14] = [
    Operator::Eq,
    Operator::Ne,
    Operator::In,
    Operator::NotIn,
    Operator::Contains,
    Operator::NotContains,
    Operator::Gt,
    Operator::Gte,
    Operator::Lt,
    Operator::Lte,
    Operator::Exists,
    Operator::NotExists,
    Operator::Matches,
    Operator::StartsWith,
];

impl Operator {
    /// The wire name of the operator.
    pub const fn name(self) -> &'static str {
        match self {
            Operator::Eq => "eq",
            Operator::Ne => "ne",
            Operator::In => "in",
            Operator::NotIn => "not_in",
            Operator::Contains => "contains",
            Operator::NotContains => "not_contains",
            Operator::Gt => "gt",
            Operator::Gte => "gte",
            Operator::Lt => "lt",
            Operator::Lte => "lte",
            Operator::Exists => "exists",
            Operator::NotExists => "not_exists",
            Operator::Matches => "matches",
            Operator::StartsWith => "starts_with",
        }
    }

    /// The operator with this wire name.
    pub fn parse(name: &str) -> Option<Self> {
        OPERATORS.into_iter().find(|op| op.name() == name)
    }
}

/// Why a condition does not validate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionError {
    /// A Pydantic `ValidationError`: the document has the wrong shape or values.
    Invalid(String),
    /// A `matches` pattern that does not compile (`re.error` in Python).
    BadPattern(String),
}

/// One predicate: compare the value at `field` in the context with `value` using `op`.
#[derive(Debug, Clone, PartialEq)]
pub struct Predicate {
    pub field: String,
    pub op: Operator,
    pub value: Value,
}

/// A validated condition tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub all: Option<Vec<Condition>>,
    pub any: Option<Vec<Condition>>,
    pub not: Option<Box<Condition>>,
    pub predicate: Option<Predicate>,
}

/// One predicate's outcome, as the run's `condition_trace` records it.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceEntry {
    pub field: String,
    pub op: Operator,
    pub expected: Value,
    /// `None` when the path is missing from the context.
    pub actual: Option<Value>,
    pub result: bool,
}

impl TraceEntry {
    /// The JSON form stored on the run row.
    pub fn to_json(&self) -> Value {
        json!({
            "field": self.field,
            "op": self.op.name(),
            "expected": self.expected,
            "actual": self.actual.clone().unwrap_or(Value::Null),
            "result": self.result,
        })
    }
}

/// The value at a dotted path, through objects only. `None` is the missing marker.
pub fn resolve<'a>(context: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = context;
    for part in path.split('.') {
        match current {
            Value::Object(map) => current = map.get(part)?,
            _ => return None,
        }
    }
    Some(current)
}

/// `^[a-z_][a-z0-9_]*(\.[a-z0-9_]+)*$`, checked without a regex.
fn valid_field(field: &str) -> bool {
    let mut segments = field.split('.');
    let first_ok = segments.next().is_some_and(|first| {
        let mut chars = first.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    });
    first_ok
        && segments.all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// Keeps the first error with the strongest precedence: a bad pattern beats a shape error,
/// because Python raises it immediately while the shape errors are collected.
fn combine<T>(results: Vec<Result<T, ConditionError>>) -> Result<Vec<T>, ConditionError> {
    let mut values = Vec::with_capacity(results.len());
    let mut invalid: Option<ConditionError> = None;
    for result in results {
        match result {
            Ok(value) => values.push(value),
            Err(error @ ConditionError::BadPattern(_)) => return Err(error),
            Err(error) => invalid = invalid.or(Some(error)),
        }
    }
    match invalid {
        Some(error) => Err(error),
        None => Ok(values),
    }
}

fn predicate_from(map: &Map<String, Value>) -> Result<Predicate, ConditionError> {
    let invalid = |message: &str| ConditionError::Invalid(message.to_owned());
    if let Some(extra) = map
        .keys()
        .find(|key| !matches!(key.as_str(), "field" | "op" | "value"))
    {
        return Err(invalid(&format!("extra field {extra} is not permitted")));
    }
    let field = match map.get("field") {
        Some(Value::String(field)) if valid_field(field) => field.clone(),
        _ => return Err(invalid("field must be a dotted lowercase path")),
    };
    let op = match map.get("op") {
        Some(Value::String(name)) => {
            Operator::parse(name).ok_or_else(|| invalid("unknown operator"))?
        }
        _ => return Err(invalid("op must be an operator name")),
    };
    let value = map.get("value").cloned().unwrap_or(Value::Null);
    if let Value::String(text) = &value
        && text.chars().count() > MAX_VALUE_CHARS
    {
        return Err(invalid("condition values are limited to 500 characters"));
    }
    if op == Operator::Matches {
        let Value::String(pattern) = &value else {
            return Err(invalid("matches requires a string pattern"));
        };
        if Regex::new(pattern).is_err() {
            return Err(ConditionError::BadPattern(pattern.clone()));
        }
    }
    Ok(Predicate { field, op, value })
}

fn list_of_conditions(value: &Value) -> Result<Vec<Condition>, ConditionError> {
    let Value::Array(items) = value else {
        return Err(ConditionError::Invalid(
            "expected a list of conditions".to_owned(),
        ));
    };
    combine(items.iter().map(validate).collect())
}

/// Validates a condition document (`Condition.model_validate`). `null` is not a condition here;
/// callers treat an absent condition as always true.
pub fn validate(value: &Value) -> Result<Condition, ConditionError> {
    let Value::Object(map) = value else {
        return Err(ConditionError::Invalid(
            "condition must be an object".to_owned(),
        ));
    };
    if map.contains_key("field") && map.contains_key("op") {
        let predicate = predicate_from(map)?;
        return Ok(Condition {
            all: None,
            any: None,
            not: None,
            predicate: Some(predicate),
        });
    }
    let all = optional_list(map.get("all"));
    let any = optional_list(map.get("any"));
    let not = match map.get("not") {
        None | Some(Value::Null) => Ok(None),
        Some(inner) => validate(inner).map(|condition| Some(Box::new(condition))),
    };
    let predicate = match map.get("predicate") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(inner)) => predicate_from(inner).map(Some),
        Some(_) => Err(ConditionError::Invalid(
            "predicate must be an object".to_owned(),
        )),
    };
    // Python raises a bad pattern at once, while shape errors are collected: a pattern wins.
    let errors: Vec<ConditionError> = [
        all.as_ref().err(),
        any.as_ref().err(),
        not.as_ref().err(),
        predicate.as_ref().err(),
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect();
    if let Some(error) = errors
        .iter()
        .find(|error| matches!(error, ConditionError::BadPattern(_)))
    {
        return Err(error.clone());
    }
    if let Some(error) = errors.into_iter().next() {
        return Err(error);
    }
    if let Some(key) = map
        .keys()
        .find(|key| !matches!(key.as_str(), "all" | "any" | "not" | "predicate"))
    {
        return Err(ConditionError::Invalid(format!(
            "extra field {key} is not permitted"
        )));
    }
    Ok(Condition {
        all: all.unwrap_or(None),
        any: any.unwrap_or(None),
        not: not.unwrap_or(None),
        predicate: predicate.unwrap_or(None),
    })
}

/// An optional `all`/`any` group: absent or null is `None`, otherwise a list of conditions.
fn optional_list(value: Option<&Value>) -> Result<Option<Vec<Condition>>, ConditionError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(items) => list_of_conditions(items).map(Some),
    }
}

fn compare_op(actual: &Value, op: Operator, expected: &Value) -> bool {
    let ordering = compare(actual, expected);
    match op {
        Operator::Eq => eq(actual, expected),
        Operator::Ne => !eq(actual, expected),
        Operator::In => contains(&container_or_empty(expected), actual).unwrap_or(false),
        Operator::NotIn => {
            contains(&container_or_empty(expected), actual).is_some_and(|found| !found)
        }
        Operator::Contains => match actual {
            Value::Array(_) | Value::String(_) | Value::Object(_) => {
                contains(actual, expected).unwrap_or(false)
            }
            _ => false,
        },
        Operator::NotContains => match actual {
            Value::Array(_) | Value::String(_) | Value::Object(_) => {
                contains(actual, expected).is_some_and(|found| !found)
            }
            _ => true,
        },
        Operator::Gt => ordering.is_some_and(std::cmp::Ordering::is_gt),
        Operator::Gte => ordering.is_some_and(std::cmp::Ordering::is_ge),
        Operator::Lt => ordering.is_some_and(std::cmp::Ordering::is_lt),
        Operator::Lte => ordering.is_some_and(std::cmp::Ordering::is_le),
        Operator::StartsWith => match actual {
            Value::String(text) => text.starts_with(crate::py::str_of(expected).as_str()),
            _ => false,
        },
        Operator::Matches => match actual {
            Value::String(text) => {
                let prefix: String = text.chars().take(MATCH_PREFIX_CHARS).collect();
                Regex::new(&crate::py::str_of(expected))
                    .map(|pattern| pattern.is_match(&prefix))
                    .unwrap_or(false)
            }
            _ => false,
        },
        // Handled before the match on `op` (they treat a missing path specially).
        Operator::Exists | Operator::NotExists => false,
    }
}

/// `expected or []`: a falsy expected value is treated as an empty list.
fn container_or_empty(expected: &Value) -> Value {
    if truthy(expected) {
        expected.clone()
    } else {
        Value::Array(Vec::new())
    }
}

/// Evaluates one predicate against the context, recording the outcome in `trace`.
fn evaluate_predicate(predicate: &Predicate, context: &Value, trace: &mut Vec<TraceEntry>) -> bool {
    let actual = resolve(context, &predicate.field);
    let result = match predicate.op {
        Operator::Exists => actual.is_some_and(|value| !value.is_null()),
        Operator::NotExists => actual.is_none_or(Value::is_null),
        op => match actual {
            None => false,
            Some(value) => compare_op(value, op, &predicate.value),
        },
    };
    trace.push(TraceEntry {
        field: predicate.field.clone(),
        op: predicate.op,
        expected: predicate.value.clone(),
        actual: actual.cloned(),
        result,
    });
    result
}

/// Evaluates a validated condition. An absent condition is true. Each predicate reached is
/// appended to `trace`; short-circuiting stops the trace where Python's does.
pub fn evaluate(
    condition: Option<&Condition>,
    context: &Value,
    trace: &mut Vec<TraceEntry>,
) -> bool {
    let Some(condition) = condition else {
        return true;
    };
    if let Some(predicate) = &condition.predicate {
        return evaluate_predicate(predicate, context, trace);
    }
    if let Some(all) = &condition.all {
        return all
            .iter()
            .all(|child| evaluate(Some(child), context, trace));
    }
    if let Some(any) = &condition.any {
        return any
            .iter()
            .any(|child| evaluate(Some(child), context, trace));
    }
    if let Some(inner) = &condition.not {
        return !evaluate(Some(inner), context, trace);
    }
    true
}
