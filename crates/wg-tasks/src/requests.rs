//! The generic request helpers of the base layer: the request models are built from the
//! OpenAPI component schemas and validated with `wg-validate`. Features read their request
//! bodies through these functions.

use serde_json::{Map, Value};
use wg_types::{ErrorCode, WgError};
use wg_validate::{
    Check, Field, LoadOptions, Model, Schema, ValidationError, model_from_json_schema, validate,
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
        ("WorkspaceCreate" | "ProjectCreate" | "ProjectUpdate", "slug") => Some(Check::Slug),
        ("WorkspaceCreate", "key_prefix") => Some(Check::KeyPrefix),
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
pub fn component(name: &str, order: &[&str]) -> Result<Model, WgError> {
    component_with(name, order, true)
}

/// [`component`], with `strip` saying whether the model strips whitespace (`InputSchema` does, a
/// plain pydantic model does not).
pub fn component_with(name: &str, order: &[&str], strip: bool) -> Result<Model, WgError> {
    let openapi = wg_wire::contract::openapi()?;
    let schema = &openapi["components"]["schemas"][name];
    let model = model_from_json_schema(
        &openapi,
        schema,
        LoadOptions {
            strip_whitespace: strip,
        },
    )
    .map_err(|err| WgError::new(ErrorCode::WorkgraphError, format!("{name}: {err}")))?;
    Ok(with_checks(model.ordered(order)))
}

pub fn run(model: &Model, body: &Value) -> Result<Map<String, Value>, WgError> {
    match validate(model, body) {
        Ok(Value::Object(dump)) => Ok(dump),
        Ok(_) => Err(WgError::new(
            ErrorCode::WorkgraphError,
            "a request model did not give an object",
        )),
        Err(errors) => Err(validation_failure(&errors)),
    }
}
