//! Validation in Pydantic's lax mode, as `Model.model_validate(payload)` runs it.

use regex::Regex;
use serde_json::{Map, Number, Value};

use crate::checks;
use crate::datetime;
use crate::error::{Loc, ValidationError, ctx, error};
use crate::schema::{ChoiceKind, Extra, FloatRules, IntRules, Model, Schema, StrRules};

type Outcome = Result<Value, Vec<ValidationError>>;

/// Validates `input` against `model`. On success the result is what `model_dump(mode="json")`
/// gives; otherwise every error, in Pydantic's order (fields in declaration order, then unknown
/// keys).
pub fn validate(model: &Model, input: &Value) -> Result<Value, Vec<ValidationError>> {
    model_value(model, input, &[])
}

/// The key of the object that stands for a non-finite float. JSON has no form for `NaN` or
/// `Infinity`, which `json.loads` still reads as floats, and a `serde_json::Value` cannot hold
/// one either: the request decoder writes a one-key object under this key, and the validator
/// reads it back as the float. A client object with exactly this key is taken for one.
pub const NON_FINITE_KEY: &str = "\u{0}non-finite";

/// The marker object for the non-finite float `number` (`NaN`, `inf` or `-inf`).
pub fn non_finite_marker(number: f64) -> Value {
    let text = if number.is_nan() {
        "NaN"
    } else if number > 0.0 {
        "Infinity"
    } else {
        "-Infinity"
    };
    let mut object = Map::new();
    object.insert(NON_FINITE_KEY.to_owned(), Value::String(text.to_owned()));
    Value::Object(object)
}

/// The float a marker object stands for, or `None` for any other value.
fn marker_number(input: &Value) -> Option<f64> {
    let object = input.as_object()?;
    if object.len() != 1 {
        return None;
    }
    match object.get(NON_FINITE_KEY)?.as_str()? {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => None,
    }
}

/// Whether `input` holds a non-finite float anywhere in it. FastAPI renders an error list with
/// `allow_nan=False`, so an error whose `input` holds one fails the request with a 500, not a 422.
pub fn holds_non_finite(input: &Value) -> bool {
    marker_number(input).is_some()
        || match input {
            Value::Array(items) => items.iter().any(holds_non_finite),
            Value::Object(map) => map.values().any(holds_non_finite),
            _ => false,
        }
}

fn one(err: ValidationError) -> Vec<ValidationError> {
    vec![err]
}

fn value(schema: &Schema, input: &Value, loc: &[Loc]) -> Outcome {
    if let Some(number) = marker_number(input) {
        return non_finite(schema, number, input, loc);
    }
    match schema {
        Schema::Any => Ok(input.clone()),
        Schema::Nullable(inner) => {
            if input.is_null() {
                Ok(Value::Null)
            } else {
                value(inner, input, loc)
            }
        }
        Schema::Str(rules) => string(rules, input, loc),
        Schema::Int(rules) => integer(rules, input, loc),
        Schema::Float(rules) => float(rules, input, loc),
        Schema::Bool => boolean(input, loc),
        Schema::DateTime => date_time(input, loc),
        Schema::Choice { kind, values } => choice(*kind, values, input, loc),
        Schema::List {
            item,
            min_length,
            max_length,
        } => list(item, *min_length, *max_length, input, loc),
        Schema::Dict(item) => dict(item.as_deref(), input, loc),
        Schema::Model(model) => model_value(model, input, loc),
    }
}

fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

fn string(rules: &StrRules, input: &Value, loc: &[Loc]) -> Outcome {
    let Some(text) = input.as_str() else {
        return Err(one(error(
            "string_type",
            "Input should be a valid string",
            loc,
            input,
            None,
        )));
    };
    let text = if rules.strip { text.trim() } else { text };
    let length = text.chars().count();
    if let Some(min) = rules.min_length
        && length < min
    {
        return Err(one(error(
            "string_too_short",
            format!("String should have at least {}", plural(min, "character")),
            loc,
            input,
            ctx([("min_length", Value::from(min))]),
        )));
    }
    if let Some(max) = rules.max_length
        && length > max
    {
        return Err(one(error(
            "string_too_long",
            format!("String should have at most {}", plural(max, "character")),
            loc,
            input,
            ctx([("max_length", Value::from(max))]),
        )));
    }
    if let Some(pattern) = &rules.pattern
        && !Regex::new(pattern).is_ok_and(|re| re.is_match(text))
    {
        return Err(one(error(
            "string_pattern_mismatch",
            format!("String should match pattern '{pattern}'"),
            loc,
            input,
            ctx([("pattern", Value::String(pattern.clone()))]),
        )));
    }
    Ok(Value::String(text.to_string()))
}

/// Pydantic prints a float bound the way Rust's `Display` does: `100`, not `100.0`.
fn bound_text(bound: f64) -> String {
    format!("{bound}")
}

fn int_value(input: &Value, loc: &[Loc]) -> Result<i64, Vec<ValidationError>> {
    let parsing = |msg: &str, kind: &str| one(error(kind, msg, loc, input, None));
    match input {
        Value::Bool(flag) => Ok(i64::from(*flag)),
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                return Ok(int);
            }
            match number.as_f64() {
                Some(float)
                    if float.is_finite() && float.fract() == 0.0 && float.abs() < 9.2e18 =>
                {
                    Ok(float as i64)
                }
                _ => Err(parsing(
                    "Input should be a valid integer, got a number with a fractional part",
                    "int_from_float",
                )),
            }
        }
        Value::String(text) => {
            let text = text.trim();
            if let Ok(int) = text.parse::<i64>() {
                return Ok(int);
            }
            match text.parse::<f64>() {
                Ok(float) if float.is_finite() && float.fract() == 0.0 && float.abs() < 9.2e18 => {
                    Ok(float as i64)
                }
                _ => Err(parsing(
                    "Input should be a valid integer, unable to parse string as an integer",
                    "int_parsing",
                )),
            }
        }
        _ => Err(parsing("Input should be a valid integer", "int_type")),
    }
}

fn integer(rules: &IntRules, input: &Value, loc: &[Loc]) -> Outcome {
    let int = int_value(input, loc)?;
    let bounds: [(&str, Option<i64>, bool, &str); 4] = [
        (
            "le",
            rules.le,
            int > rules.le.unwrap_or(i64::MAX),
            "less_than_equal",
        ),
        (
            "lt",
            rules.lt,
            int >= rules.lt.unwrap_or(i64::MAX),
            "less_than",
        ),
        (
            "ge",
            rules.ge,
            int < rules.ge.unwrap_or(i64::MIN),
            "greater_than_equal",
        ),
        (
            "gt",
            rules.gt,
            int <= rules.gt.unwrap_or(i64::MIN),
            "greater_than",
        ),
    ];
    for (key, bound, failed, kind) in bounds {
        if let (Some(bound), true) = (bound, failed) {
            return Err(one(error(
                kind,
                bound_message(key, &bound.to_string()),
                loc,
                input,
                ctx([(key, Value::from(bound))]),
            )));
        }
    }
    Ok(Value::from(int))
}

fn bound_message(key: &str, bound: &str) -> String {
    let relation = match key {
        "le" => "less than or equal to",
        "lt" => "less than",
        "ge" => "greater than or equal to",
        _ => "greater than",
    };
    format!("Input should be {relation} {bound}")
}

/// A non-finite float where the schema expects a value. `Any` takes it (Python keeps the float,
/// which the output writes as null), a float field checks its bounds, and any other type refuses it.
fn non_finite(schema: &Schema, number: f64, input: &Value, loc: &[Loc]) -> Outcome {
    match schema {
        Schema::Any => Ok(Value::Null),
        Schema::Nullable(inner) => non_finite(inner, number, input, loc),
        Schema::Float(rules) => float_bounds(rules, number, input, loc, true),
        _ => Err(one(error(
            "float_type",
            "Input should be a valid number",
            loc,
            input,
            None,
        ))),
    }
}

fn float(rules: &FloatRules, input: &Value, loc: &[Loc]) -> Outcome {
    let number = match input {
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => text.trim().parse::<f64>().map_err(|_| {
            one(error(
                "float_parsing",
                "Input should be a valid number, unable to parse string as a number",
                loc,
                input,
                None,
            ))
        })?,
        _ => {
            return Err(one(error(
                "float_type",
                "Input should be a valid number",
                loc,
                input,
                None,
            )));
        }
    };
    float_bounds(rules, number, input, loc, false)
}

/// The bounds of a float field. A NaN fails every bound it has when `nan_fails` (Python's
/// comparisons with NaN are false, so pydantic rejects it), and passes when there are none.
fn float_bounds(
    rules: &FloatRules,
    number: f64,
    input: &Value,
    loc: &[Loc],
    nan_fails: bool,
) -> Outcome {
    let nan = nan_fails && number.is_nan();
    let bounds: [(&str, Option<f64>, bool, &str); 4] = [
        (
            "le",
            rules.le,
            nan || number > rules.le.unwrap_or(f64::INFINITY),
            "less_than_equal",
        ),
        (
            "lt",
            rules.lt,
            nan || number >= rules.lt.unwrap_or(f64::INFINITY),
            "less_than",
        ),
        (
            "ge",
            rules.ge,
            nan || number < rules.ge.unwrap_or(f64::NEG_INFINITY),
            "greater_than_equal",
        ),
        (
            "gt",
            rules.gt,
            nan || number <= rules.gt.unwrap_or(f64::NEG_INFINITY),
            "greater_than",
        ),
    ];
    for (key, bound, failed, kind) in bounds {
        if let (Some(bound), true) = (bound, failed) {
            return Err(one(error(
                kind,
                bound_message(key, &bound_text(bound)),
                loc,
                input,
                ctx([(
                    key,
                    Number::from_f64(bound).map_or(Value::Null, Value::Number),
                )]),
            )));
        }
    }
    Ok(Number::from_f64(number).map_or(Value::Null, Value::Number))
}

fn boolean(input: &Value, loc: &[Loc]) -> Outcome {
    let parsing = || {
        one(error(
            "bool_parsing",
            "Input should be a valid boolean, unable to interpret input",
            loc,
            input,
            None,
        ))
    };
    match input {
        Value::Bool(_) => Ok(input.clone()),
        Value::Number(number) => match number.as_f64() {
            Some(0.0) => Ok(Value::Bool(false)),
            Some(1.0) => Ok(Value::Bool(true)),
            _ if number.is_i64() || number.is_u64() => Err(parsing()),
            _ => Err(one(error(
                "bool_type",
                "Input should be a valid boolean",
                loc,
                input,
                None,
            ))),
        },
        Value::String(text) => match text.to_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(Value::Bool(false)),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(Value::Bool(true)),
            _ => Err(parsing()),
        },
        _ => Err(one(error(
            "bool_type",
            "Input should be a valid boolean",
            loc,
            input,
            None,
        ))),
    }
}

fn date_time(input: &Value, loc: &[Loc]) -> Outcome {
    let invalid = || {
        one(error(
            "datetime_type",
            "Input should be a valid datetime",
            loc,
            input,
            None,
        ))
    };
    match input {
        Value::String(text) => match datetime::parse(text.trim()) {
            Ok(parsed) => Ok(Value::String(parsed.to_iso())),
            Err(reason) => Err(one(error(
                "datetime_from_date_parsing",
                format!("Input should be a valid datetime or date, {reason}"),
                loc,
                input,
                ctx([("error", Value::String(reason.to_string()))]),
            ))),
        },
        Value::Number(number) => number
            .as_f64()
            .and_then(datetime::from_timestamp)
            .map(|parsed| Value::String(parsed.to_iso()))
            .ok_or_else(invalid),
        _ => Err(invalid()),
    }
}

fn quoted_list(values: &[String]) -> String {
    let quoted: Vec<String> = values.iter().map(|v| format!("'{v}'")).collect();
    match quoted.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}

fn choice(kind: ChoiceKind, values: &[String], input: &Value, loc: &[Loc]) -> Outcome {
    if let Some(text) = input.as_str()
        && values.iter().any(|v| v == text)
    {
        return Ok(input.clone());
    }
    let expected = quoted_list(values);
    let kind_name = match kind {
        ChoiceKind::Literal => "literal_error",
        ChoiceKind::Enum => "enum",
    };
    Err(one(error(
        kind_name,
        format!("Input should be {expected}"),
        loc,
        input,
        ctx([("expected", Value::String(expected.clone()))]),
    )))
}

fn list(
    item: &Schema,
    min_length: Option<usize>,
    max_length: Option<usize>,
    input: &Value,
    loc: &[Loc],
) -> Outcome {
    let Some(items) = input.as_array() else {
        return Err(one(error(
            "list_type",
            "Input should be a valid list",
            loc,
            input,
            None,
        )));
    };
    let length_error = |kind: &str, word: &str, limit: usize, key: &str| {
        one(error(
            kind,
            format!(
                "List should have at {word} {} after validation, not {}",
                plural(limit, "item"),
                items.len()
            ),
            loc,
            input,
            ctx([
                ("field_type", Value::String("List".to_string())),
                (key, Value::from(limit)),
                ("actual_length", Value::from(items.len())),
            ]),
        ))
    };
    if let Some(max) = max_length
        && items.len() > max
    {
        return Err(length_error("too_long", "most", max, "max_length"));
    }
    let mut errors = Vec::new();
    let mut out = Vec::new();
    for (index, element) in items.iter().enumerate() {
        let mut item_loc = loc.to_vec();
        item_loc.push(Loc::Index(index));
        match value(item, element, &item_loc) {
            Ok(done) => out.push(done),
            Err(found) => errors.extend(found),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if let Some(min) = min_length
        && items.len() < min
    {
        return Err(length_error("too_short", "least", min, "min_length"));
    }
    Ok(Value::Array(out))
}

fn dict(item: Option<&Schema>, input: &Value, loc: &[Loc]) -> Outcome {
    let Some(map) = input.as_object() else {
        return Err(one(error(
            "dict_type",
            "Input should be a valid dictionary",
            loc,
            input,
            None,
        )));
    };
    let Some(item) = item else {
        return Ok(input.clone());
    };
    let mut errors = Vec::new();
    let mut out = Map::new();
    for (key, element) in map {
        let mut item_loc = loc.to_vec();
        item_loc.push(Loc::Key(key.clone()));
        match value(item, element, &item_loc) {
            Ok(done) => {
                out.insert(key.clone(), done);
            }
            Err(found) => errors.extend(found),
        }
    }
    if errors.is_empty() {
        Ok(Value::Object(out))
    } else {
        Err(errors)
    }
}

fn model_value(model: &Model, input: &Value, loc: &[Loc]) -> Outcome {
    let Some(map) = input.as_object() else {
        return Err(one(error(
            "model_type",
            format!(
                "Input should be a valid dictionary or instance of {}",
                model.name
            ),
            loc,
            input,
            ctx([("class_name", Value::String(model.name.clone()))]),
        )));
    };
    let mut errors = Vec::new();
    let mut out = Map::new();
    for field in &model.fields {
        let mut field_loc = loc.to_vec();
        field_loc.push(Loc::Key(field.name.clone()));
        let Some(given) = map.get(&field.name) else {
            match &field.default {
                Some(default) => {
                    out.insert(field.name.clone(), default.clone());
                }
                None => errors.push(error("missing", "Field required", &field_loc, input, None)),
            }
            continue;
        };
        let checked = value(&field.schema, given, &field_loc).and_then(|done| match field.check {
            None => Ok(done),
            Some(check) => checks::run(check, &done).map_err(|message| {
                one(error(
                    "value_error",
                    format!("Value error, {message}"),
                    &field_loc,
                    &done,
                    ctx([("error", Value::String(message.clone()))]),
                ))
            }),
        });
        match checked {
            Ok(done) => {
                out.insert(field.name.clone(), done);
            }
            Err(found) => errors.extend(found),
        }
    }
    if model.extra == Extra::Forbid {
        for (key, extra) in map {
            if model.fields.iter().all(|field| &field.name != key) {
                let mut extra_loc = loc.to_vec();
                extra_loc.push(Loc::Key(key.clone()));
                errors.push(error(
                    "extra_forbidden",
                    "Extra inputs are not permitted",
                    &extra_loc,
                    extra,
                    None,
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(Value::Object(out))
    } else {
        Err(errors)
    }
}
