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

fn one(err: ValidationError) -> Vec<ValidationError> {
    vec![err]
}

fn value(schema: &Schema, input: &Value, loc: &[Loc]) -> Outcome {
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

/// 2**63: a float at or beyond this, either way, is not an integer Pydantic will take (the lower
/// edge is exclusive too: `-2**63` is refused).
const I64_LIMIT: f64 = 9_223_372_036_854_775_808.0;

/// The most digits of an integer string Python converts (`sys.get_int_max_str_digits()`).
const MAX_INT_DIGITS: usize = 4300;

/// An integer as a JSON number. serde_json keeps the digits of an integer under
/// `arbitrary_precision`, so every value of the 128-bit range is exact.
fn int_json(int: i128) -> Value {
    serde_json::from_str(&int.to_string()).unwrap_or(Value::Null)
}

/// Whether the text is an optional minus sign and ASCII digits, as serde_json writes an integer.
fn is_integer_literal(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The integer literal Pydantic's lax mode reads from a string, as `int(text)` would: an optional
/// sign, ASCII digits in groups joined by single underscores, and an optional fraction of zeros
/// only (`"5.00"`). Exponents, other fractions, and non-ASCII digits are refused.
fn int_text(text: &str) -> Option<String> {
    let (sign, rest) = match text.as_bytes().first() {
        Some(b'-') => ("-", &text[1..]),
        Some(b'+') => ("", &text[1..]),
        _ => ("", text),
    };
    let (whole, fraction) = match rest.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rest, None),
    };
    if fraction.is_some_and(|f| f.is_empty() || !f.bytes().all(|b| b == b'0')) {
        return None;
    }
    let groups: Vec<&str> = whole.split('_').collect();
    if groups
        .iter()
        .any(|group| group.is_empty() || !group.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(format!("{sign}{}", groups.concat()))
}

fn int_value(input: &Value, loc: &[Loc]) -> Result<i128, Vec<ValidationError>> {
    let parsing = |msg: &str, kind: &str| one(error(kind, msg, loc, input, None));
    // A value past 128 bits is not held. Pydantic keeps it, so this is a known gap, reported as
    // Pydantic's size error.
    let too_big = || {
        one(error(
            "int_parsing_size",
            "Unable to parse input string as an integer, exceeded maximum size",
            loc,
            input,
            None,
        ))
    };
    match input {
        Value::Bool(flag) => Ok(i128::from(*flag)),
        Value::Number(number) => {
            let literal = number.to_string();
            if is_integer_literal(&literal) {
                return literal.parse::<i128>().map_err(|_| too_big());
            }
            let float = number.as_f64().unwrap_or(f64::NAN);
            if !float.is_finite() || float.fract() != 0.0 {
                return Err(parsing(
                    "Input should be a valid integer, got a number with a fractional part",
                    "int_from_float",
                ));
            }
            if float > -I64_LIMIT && float < I64_LIMIT {
                Ok(float as i128)
            } else {
                Err(too_big())
            }
        }
        Value::String(text) => {
            let literal = int_text(text.trim()).ok_or_else(|| {
                parsing(
                    "Input should be a valid integer, unable to parse string as an integer",
                    "int_parsing",
                )
            })?;
            let digits = literal.trim_start_matches(['-', '+']).len();
            if digits > MAX_INT_DIGITS {
                return Err(too_big());
            }
            literal.parse::<i128>().map_err(|_| too_big())
        }
        _ => Err(parsing("Input should be a valid integer", "int_type")),
    }
}

fn integer(rules: &IntRules, input: &Value, loc: &[Loc]) -> Outcome {
    let int = int_value(input, loc)?;
    let bounds: [(&str, Option<i128>, bool, &str); 4] = [
        (
            "le",
            rules.le,
            int > rules.le.unwrap_or(i128::MAX),
            "less_than_equal",
        ),
        (
            "lt",
            rules.lt,
            int >= rules.lt.unwrap_or(i128::MAX),
            "less_than",
        ),
        (
            "ge",
            rules.ge,
            int < rules.ge.unwrap_or(i128::MIN),
            "greater_than_equal",
        ),
        (
            "gt",
            rules.gt,
            int <= rules.gt.unwrap_or(i128::MIN),
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
                ctx([(key, int_json(bound))]),
            )));
        }
    }
    Ok(int_json(int))
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
    let bounds: [(&str, Option<f64>, bool, &str); 4] = [
        (
            "le",
            rules.le,
            number > rules.le.unwrap_or(f64::INFINITY),
            "less_than_equal",
        ),
        (
            "lt",
            rules.lt,
            number >= rules.lt.unwrap_or(f64::INFINITY),
            "less_than",
        ),
        (
            "ge",
            rules.ge,
            number < rules.ge.unwrap_or(f64::NEG_INFINITY),
            "greater_than_equal",
        ),
        (
            "gt",
            rules.gt,
            number <= rules.gt.unwrap_or(f64::NEG_INFINITY),
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
