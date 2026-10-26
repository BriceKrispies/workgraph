//! Tool arguments checked against the tool's input schema, with Pydantic's error text.
//!
//! The Python SDK validates a call's `arguments` with a Pydantic model named
//! `<tool>Arguments` and, on failure, answers with `str(ValidationError)` (formatted by
//! `wg_validate::pydantic`).

use serde_json::{Map, Value};
use wg_validate::{ValidationError, pydantic::error_text, validate};

use crate::catalog::ToolSpec;

/// The arguments did not validate.
#[derive(Clone, Debug, PartialEq)]
pub struct ArgumentsError {
    /// The model title, `<tool>Arguments`.
    pub title: String,
    pub errors: Vec<ValidationError>,
    /// The whole arguments object, which a `missing` error reports as its input.
    pub input: Value,
}

/// Validates `arguments` (absent or `null` means no arguments) for `spec`. On success the value is
/// the validated arguments with defaults filled in.
pub fn check_arguments(
    spec: &ToolSpec,
    arguments: Option<&Value>,
) -> Result<Value, ArgumentsError> {
    let empty = Value::Object(Map::new());
    let input = match arguments {
        None | Some(Value::Null) => &empty,
        Some(value) => value,
    };
    validate(&spec.arguments, input).map_err(|errors| ArgumentsError {
        title: spec.arguments.name.clone(),
        errors,
        input: input.clone(),
    })
}

impl ArgumentsError {
    /// `str(ValidationError)`.
    pub fn text(&self) -> String {
        error_text(&self.title, &self.errors)
    }
}
