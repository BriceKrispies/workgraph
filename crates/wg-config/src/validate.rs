//! Validation of a loaded value against a [`Model`], in Pydantic's lax mode, with Pydantic's error
//! types, messages and locations.
//!
//! Errors are collected in the order Pydantic reports them: a model's fields in declaration order,
//! then its extra keys in document order; list items and dict entries in order; a tagged union's
//! member errors below the tag in the location.

use serde_json::{Map, Value};
use wg_validate::{Loc, ValidationError};

use crate::netaddr::is_network;
use crate::pyfmt::{bound_text, one_of, repr_str};
use crate::schema::{Model, Schema};

/// Validates `input` against `model`. Returns the model's dump (defaults filled in, declaration
/// order) when there are no errors.
pub(crate) fn validate_model(
    model: &Model,
    input: &Value,
    loc: &[Loc],
    errors: &mut Vec<ValidationError>,
) -> Option<Map<String, Value>> {
    let Some(object) = input.as_object() else {
        let message = format!(
            "Input should be a valid dictionary or instance of {}",
            model.name
        );
        push(errors, "model_type", message, loc, input);
        return None;
    };
    let mut dump = Map::new();
    let mut valid = true;
    for field in &model.fields {
        let field_loc = child(loc, Loc::Key(field.name.to_string()));
        let value = match object.get(field.name) {
            Some(given) => validate(&field.schema, given, &field_loc, errors),
            None => match &field.default {
                Some(default) => validate(&field.schema, default, &field_loc, errors),
                None => {
                    push(
                        errors,
                        "missing",
                        "Field required".to_string(),
                        &field_loc,
                        input,
                    );
                    None
                }
            },
        };
        match value {
            Some(value) => {
                dump.insert(field.name.to_string(), value);
            }
            None => valid = false,
        }
    }
    for (key, given) in object {
        if !model.fields.iter().any(|field| field.name == key) {
            let key_loc = child(loc, Loc::Key(key.clone()));
            push(
                errors,
                "extra_forbidden",
                "Extra inputs are not permitted".to_string(),
                &key_loc,
                given,
            );
            valid = false;
        }
    }
    valid.then_some(dump)
}

/// Validates one value against one schema.
pub(crate) fn validate(
    schema: &Schema,
    input: &Value,
    loc: &[Loc],
    errors: &mut Vec<ValidationError>,
) -> Option<Value> {
    match schema {
        Schema::Any => Some(input.clone()),
        Schema::Str => string(input, loc, errors).map(|text| Value::String(text.to_string())),
        Schema::NonBlank { message } => {
            let text = string(input, loc, errors)?;
            if text.trim().is_empty() {
                let message = format!("Value error, {message}");
                push(errors, "value_error", message, loc, input);
                None
            } else {
                Some(Value::String(text.to_string()))
            }
        }
        Schema::Int { ge, le } => {
            let number = integer(input, loc, errors)?;
            if let Some(bound) = ge.filter(|bound| number < *bound) {
                let message = format!("Input should be greater than or equal to {bound}");
                push_ctx(
                    errors,
                    "greater_than_equal",
                    message,
                    loc,
                    input,
                    Some(bound_ctx("ge", Value::from(bound))),
                );
                return None;
            }
            if let Some(bound) = le.filter(|bound| number > *bound) {
                let message = format!("Input should be less than or equal to {bound}");
                push_ctx(
                    errors,
                    "less_than_equal",
                    message,
                    loc,
                    input,
                    Some(bound_ctx("le", Value::from(bound))),
                );
                return None;
            }
            Some(Value::from(number))
        }
        Schema::Float { ge, gt } => {
            let number = float(input, loc, errors)?;
            if let Some(bound) = ge.filter(|bound| number < *bound) {
                let message = format!(
                    "Input should be greater than or equal to {}",
                    bound_text(bound)
                );
                push_ctx(
                    errors,
                    "greater_than_equal",
                    message,
                    loc,
                    input,
                    Some(bound_ctx("ge", float_json(bound))),
                );
                return None;
            }
            if let Some(bound) = gt.filter(|bound| number <= *bound) {
                let message = format!("Input should be greater than {}", bound_text(bound));
                push_ctx(
                    errors,
                    "greater_than",
                    message,
                    loc,
                    input,
                    Some(bound_ctx("gt", float_json(bound))),
                );
                return None;
            }
            Some(Value::from(number))
        }
        Schema::Bool => boolean(input, loc, errors).map(Value::Bool),
        Schema::Literal(values) => match input {
            Value::String(text) if values.contains(&text.as_str()) => Some(input.clone()),
            _ => {
                let message = format!("Input should be {}", one_of(values.iter().copied()));
                push(errors, "literal_error", message, loc, input);
                None
            }
        },
        Schema::Nullable(inner) => {
            if input.is_null() {
                Some(Value::Null)
            } else {
                validate(inner, input, loc, errors)
            }
        }
        Schema::List { item, min_len } => {
            let items = list(input, loc, errors, item)?;
            if items.len() < *min_len {
                let message = format!(
                    "List should have at least {min_len} item{}",
                    if *min_len == 1 { "" } else { "s" }
                );
                push_ctx(
                    errors,
                    "too_short",
                    message,
                    loc,
                    input,
                    Some(list_length_ctx(*min_len, items.len())),
                );
                return None;
            }
            Some(Value::Array(items))
        }
        Schema::Networks => {
            let items = list(input, loc, errors, &Schema::Str)?;
            for item in &items {
                let text = item.as_str().unwrap_or_default();
                if !is_network(text) {
                    let message = format!(
                        "Value error, {} does not appear to be an IPv4 or IPv6 network",
                        repr_str(text)
                    );
                    push(errors, "value_error", message, loc, input);
                    return None;
                }
            }
            Some(Value::Array(items))
        }
        Schema::JiraProjects => {
            let items = list(input, loc, errors, &Schema::Str)?;
            if items.is_empty() {
                push_ctx(
                    errors,
                    "too_short",
                    "List should have at least 1 item".to_string(),
                    loc,
                    input,
                    Some(list_length_ctx(1, 0)),
                );
                return None;
            }
            for item in &items {
                let text = item.as_str().unwrap_or_default();
                if !is_jira_key(text) {
                    let message =
                        format!("Value error, invalid Jira project key {}", repr_str(text));
                    push(errors, "value_error", message, loc, input);
                    return None;
                }
            }
            Some(Value::Array(items))
        }
        Schema::Dict(inner) => {
            let Some(members) = input.as_object() else {
                push(
                    errors,
                    "dict_type",
                    "Input should be a valid dictionary".to_string(),
                    loc,
                    input,
                );
                return None;
            };
            let mut out = Map::new();
            let mut valid = true;
            for (key, given) in members {
                match validate(inner, given, &child(loc, Loc::Key(key.clone())), errors) {
                    Some(value) => {
                        out.insert(key.clone(), value);
                    }
                    None => valid = false,
                }
            }
            valid.then_some(Value::Object(out))
        }
        Schema::Model(model) => validate_model(model, input, loc, errors).map(Value::Object),
        Schema::Tagged(members) => tagged(members, input, loc, errors),
        Schema::Checked(inner, check) => {
            let dump = validate(inner, input, loc, errors)?;
            match (check.0)(&dump) {
                None => Some(dump),
                Some(message) => {
                    push(
                        errors,
                        "value_error",
                        format!("Value error, {message}"),
                        loc,
                        input,
                    );
                    None
                }
            }
        }
    }
}

/// A discriminated union: the member is the one named by the `type` key, and its errors sit below
/// that tag in the location.
fn tagged(
    members: &[(&'static str, Model)],
    input: &Value,
    loc: &[Loc],
    errors: &mut Vec<ValidationError>,
) -> Option<Value> {
    let Some(object) = input.as_object() else {
        push(
            errors,
            "dict_type",
            "Input should be a valid dictionary".to_string(),
            loc,
            input,
        );
        return None;
    };
    let expected: Vec<&str> = members.iter().map(|(tag, _)| *tag).collect();
    let Some(tag_value) = object.get("type") else {
        push(
            errors,
            "union_tag_not_found",
            "Unable to extract tag using discriminator 'type'".to_string(),
            loc,
            input,
        );
        return None;
    };
    let tag = tag_value.as_str().unwrap_or_default();
    let Some((name, model)) = members.iter().find(|(name, _)| *name == tag) else {
        let listed: Vec<String> = expected.iter().map(|tag| format!("'{tag}'")).collect();
        let message = format!(
            "Input tag {} found using 'type' does not match any of the expected tags: {}",
            describe_tag(tag_value),
            listed.join(", ")
        );
        push(errors, "union_tag_invalid", message, loc, input);
        return None;
    };
    validate_model(
        model,
        input,
        &child(loc, Loc::Key((*name).to_string())),
        errors,
    )
    .map(Value::Object)
}

fn describe_tag(value: &Value) -> String {
    match value {
        Value::String(text) => repr_str(text),
        other => other.to_string(),
    }
}

fn string<'a>(input: &'a Value, loc: &[Loc], errors: &mut Vec<ValidationError>) -> Option<&'a str> {
    match input.as_str() {
        Some(text) => Some(text),
        None => {
            push(
                errors,
                "string_type",
                "Input should be a valid string".to_string(),
                loc,
                input,
            );
            None
        }
    }
}

/// Lax `int`: a number with no fraction, a boolean, or a string of digits.
fn integer(input: &Value, loc: &[Loc], errors: &mut Vec<ValidationError>) -> Option<i64> {
    match input {
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::Number(number) => match number.as_i64() {
            Some(int) => Some(int),
            None => match number.as_f64() {
                Some(float) if float.fract() == 0.0 && float.abs() < 9.0e18 => Some(float as i64),
                Some(_) => {
                    push(
                        errors,
                        "int_from_float",
                        "Input should be a valid integer, got a number with a fractional part"
                            .to_string(),
                        loc,
                        input,
                    );
                    None
                }
                None => {
                    push(
                        errors,
                        "int_type",
                        "Input should be a valid integer".to_string(),
                        loc,
                        input,
                    );
                    None
                }
            },
        },
        Value::String(text) => match text.trim().parse::<i64>() {
            Ok(int) => Some(int),
            Err(_) => {
                push(
                    errors,
                    "int_parsing",
                    "Input should be a valid integer, unable to parse string as an integer"
                        .to_string(),
                    loc,
                    input,
                );
                None
            }
        },
        _ => {
            push(
                errors,
                "int_type",
                "Input should be a valid integer".to_string(),
                loc,
                input,
            );
            None
        }
    }
}

/// Lax `float`: a number, or a string that parses as one.
fn float(input: &Value, loc: &[Loc], errors: &mut Vec<ValidationError>) -> Option<f64> {
    match input {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => match text.trim().parse::<f64>() {
            Ok(value) => Some(value),
            Err(_) => {
                push(
                    errors,
                    "float_parsing",
                    "Input should be a valid number, unable to parse string as a number"
                        .to_string(),
                    loc,
                    input,
                );
                None
            }
        },
        _ => {
            push(
                errors,
                "float_type",
                "Input should be a valid number".to_string(),
                loc,
                input,
            );
            None
        }
    }
}

/// Lax `bool`: a boolean, 0 or 1, or one of the usual true and false words.
fn boolean(input: &Value, loc: &[Loc], errors: &mut Vec<ValidationError>) -> Option<bool> {
    let parsed = match input {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => match number.as_i64() {
            Some(0) => Some(false),
            Some(1) => Some(true),
            _ => None,
        },
        Value::String(text) => match text.to_ascii_lowercase().as_str() {
            "true" | "t" | "yes" | "y" | "on" | "1" => Some(true),
            "false" | "f" | "no" | "n" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => {
            push(
                errors,
                "bool_type",
                "Input should be a valid boolean".to_string(),
                loc,
                input,
            );
            return None;
        }
    };
    if parsed.is_none() {
        push(
            errors,
            "bool_parsing",
            "Input should be a valid boolean, unable to interpret input".to_string(),
            loc,
            input,
        );
    }
    parsed
}

/// A list whose items each validate against `item`. Item errors sit below the item's index.
fn list(
    input: &Value,
    loc: &[Loc],
    errors: &mut Vec<ValidationError>,
    item: &Schema,
) -> Option<Vec<Value>> {
    let Some(items) = input.as_array() else {
        push(
            errors,
            "list_type",
            "Input should be a valid list".to_string(),
            loc,
            input,
        );
        return None;
    };
    let mut out = Vec::with_capacity(items.len());
    let mut valid = true;
    for (index, given) in items.iter().enumerate() {
        match validate(item, given, &child(loc, Loc::Index(index)), errors) {
            Some(value) => out.push(value),
            None => valid = false,
        }
    }
    valid.then_some(out)
}

/// `^[A-Z][A-Z0-9]*` after the Python checks: alphanumeric, a leading letter, and upper case.
fn is_jira_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    key.chars().all(char::is_alphanumeric) && first.is_alphabetic() && key.to_uppercase() == key
}

fn child(loc: &[Loc], step: Loc) -> Vec<Loc> {
    let mut out = loc.to_vec();
    out.push(step);
    out
}

fn push(
    errors: &mut Vec<ValidationError>,
    kind: &str,
    message: String,
    loc: &[Loc],
    input: &Value,
) {
    push_ctx(errors, kind, message, loc, input, None);
}

/// Pushes an error with Pydantic's context object (the bound or the list lengths it reports).
fn push_ctx(
    errors: &mut Vec<ValidationError>,
    kind: &str,
    message: String,
    loc: &[Loc],
    input: &Value,
    ctx: Option<Map<String, Value>>,
) {
    errors.push(ValidationError {
        kind: kind.to_string(),
        loc: loc.to_vec(),
        msg: message,
        input: input.clone(),
        ctx,
    });
}

/// A context object with one bound, such as `{"le": 65535}`.
fn bound_ctx(key: &str, bound: Value) -> Map<String, Value> {
    let mut ctx = Map::new();
    ctx.insert(key.to_string(), bound);
    ctx
}

/// The context of a `too_short` list error: `field_type`, `min_length` and `actual_length`.
fn list_length_ctx(min_length: usize, actual_length: usize) -> Map<String, Value> {
    let mut ctx = Map::new();
    ctx.insert("field_type".to_string(), Value::String("List".to_string()));
    ctx.insert("min_length".to_string(), Value::from(min_length));
    ctx.insert("actual_length".to_string(), Value::from(actual_length));
    ctx
}

/// A float bound as the JSON number Pydantic's context carries (`0.0` stays a float).
fn float_json(bound: f64) -> Value {
    serde_json::Number::from_f64(bound).map_or(Value::Null, Value::Number)
}
