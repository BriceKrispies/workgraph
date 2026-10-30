//! The argument and value readers the command groups share (`wg_clispec::args`): the clap option
//! readers over `ArgMatches`, the `@file` reader type, and the readers of the JSON values the
//! human views print. Every group reads its options and fields through them.

use clap::ArgMatches;
use wg_json::Value;
use wg_model::py_repr_str;

use crate::failure::python_number_text;
use crate::{CliFailure, Usage};

/// Reads a file named by an `@file` option: the text, or the reason it could not be read.
pub type ReadFile<'a> = &'a dyn Fn(&str) -> Result<String, String>;

pub fn one(m: &ArgMatches, id: &str) -> Option<String> {
    m.try_get_one::<String>(id).ok().flatten().cloned()
}

pub fn many(m: &ArgMatches, id: &str) -> Vec<String> {
    m.try_get_many::<String>(id)
        .ok()
        .flatten()
        .map(|values| values.cloned().collect())
        .unwrap_or_default()
}

pub fn flag(m: &ArgMatches, id: &str) -> bool {
    m.try_get_one::<bool>(id)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

/// The `int` option `id`, else `default`. Click's `int` check: a value Python's `int()` cannot read
/// is a usage error (`at` names the command it is reported under).
pub fn number(m: &ArgMatches, id: &str, default: i64, at: Usage) -> Result<i64, CliFailure> {
    match one(m, id) {
        None => Ok(default),
        Some(text) => python_number_text(&text)
            .and_then(|digits| digits.parse().ok())
            .ok_or_else(|| at.invalid_value(id, &text, "int")),
    }
}

/// The `float` option `id`, when given. Click's `float` check: Python's `float()` must read the
/// text, else it is a usage error (`at` names the command it is reported under).
pub fn float_value(m: &ArgMatches, id: &str, at: Usage) -> Result<Option<f64>, CliFailure> {
    one(m, id)
        .map(|text| {
            python_number_text(&text)
                .and_then(|digits| digits.parse().ok())
                .ok_or_else(|| at.invalid_value(id, &text, "float"))
        })
        .transpose()
}

/// The option values `task ready`, `graph` and `history` read.
#[must_use]
pub fn option_text(m: &ArgMatches, id: &str) -> Option<String> {
    one(m, id).filter(|text| !text.is_empty())
}

/// The values of a repeatable option.
#[must_use]
pub fn option_list(m: &ArgMatches, id: &str) -> Vec<String> {
    many(m, id)
}

/// A flag option.
#[must_use]
pub fn option_flag(m: &ArgMatches, id: &str) -> bool {
    flag(m, id)
}

pub fn get<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Model { fields, .. } => fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, found)| found),
        Value::Dict(entries) => entries
            .iter()
            .find(|(key, _)| python(&Value::Str(key.clone())) == name)
            .map(|(_, found)| found),
        _ => None,
    }
}

pub fn items(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::List(items)) => items,
        _ => &[],
    }
}

/// Python `str()` of a scalar output value.
pub fn python(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => wg_json::py_float_repr(*number),
        Value::Str(text) => text
            .code_points()
            .iter()
            .filter_map(|point| char::from_u32(*point))
            .collect(),
        Value::DateTime(at) => at.format_display(),
        other => repr(other),
    }
}

/// Python `repr()` of an output value, as `str()` shows the members of a list or dict.
fn repr(value: &Value) -> String {
    match value {
        Value::Str(_) => py_repr_str(&python(value)),
        Value::List(members) => {
            let inner: Vec<String> = members.iter().map(repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Dict(entries) => {
            let inner: Vec<String> = entries
                .iter()
                .map(|(key, member)| {
                    format!("{}: {}", repr(&Value::Str(key.clone())), repr(member))
                })
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        Value::Model { fields, .. } => {
            let inner: Vec<String> = fields
                .iter()
                .map(|(key, member)| format!("{}: {}", py_repr_str(key), repr(member)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        other => python(other),
    }
}

pub fn s(value: Option<&Value>) -> String {
    value.map(python).unwrap_or_default()
}

pub fn opt(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(other) => Some(python(other)),
    }
}

/// A table cell, `"" if v is None else str(v)`: the value's `str()`, or nothing for a missing or
/// null value.
pub fn cell(value: Option<&Value>) -> String {
    opt(value).unwrap_or_default()
}

/// `when()`: `YYYY-MM-DD HH:MM`, or nothing.
pub fn when(value: Option<&Value>) -> String {
    match value {
        Some(Value::DateTime(at)) => at.format_display(),
        _ => String::new(),
    }
}

pub fn count(value: Option<&Value>) -> Option<String> {
    let length = items(value).len();
    (length > 0).then(|| length.to_string())
}
