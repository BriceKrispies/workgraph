//! Models from the JSON Schema documents Pydantic generates (`contracts/openapi.json` component
//! schemas and the `input_schema` of `contracts/mcp.json` tools).

use std::fmt;

use serde_json::Value;

use crate::schema::{ChoiceKind, Extra, Field, FloatRules, IntRules, Model, Schema, StrRules};

/// Options for [`model_from_json_schema`].
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadOptions {
    /// `str_strip_whitespace`: every string is stripped before its constraints are checked.
    pub strip_whitespace: bool,
}

/// A JSON Schema construct the loader does not translate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError(pub String);

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported JSON Schema: {}", self.0)
    }
}

impl std::error::Error for LoadError {}

struct Loader<'a> {
    root: &'a Value,
    options: LoadOptions,
}

/// Translates the object schema `schema` into a [`Model`]. `$ref`s (`#/components/schemas/X`,
/// `#/$defs/X`) are resolved against `root`. A schema without `additionalProperties: false`
/// ignores unknown keys, as the MCP argument models do. Fields come in the document's key order
/// (sorted in the snapshots); use [`Model::ordered`] for declaration order.
pub fn model_from_json_schema(
    root: &Value,
    schema: &Value,
    options: LoadOptions,
) -> Result<Model, LoadError> {
    let loader = Loader { root, options };
    match loader.schema(schema)? {
        Schema::Model(model) => Ok(*model),
        other => Err(LoadError(format!("not an object model: {other:?}"))),
    }
}

fn count(schema: &Value, key: &str) -> Option<usize> {
    schema
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
}

fn int_bound(schema: &Value, key: &str) -> Option<i64> {
    let bound = schema.get(key)?;
    bound.as_i64().or_else(|| {
        // A float-typed bound (`1.0`) on an integer field, which FastAPI's schema generator emits.
        bound
            .as_f64()
            .filter(|float| float.fract() == 0.0 && float.abs() < 9.0e15)
            .map(|float| float as i64)
    })
}

fn float_bound(schema: &Value, key: &str) -> Option<f64> {
    schema.get(key).and_then(Value::as_f64)
}

impl Loader<'_> {
    fn resolve(&self, reference: &str) -> Result<&Value, LoadError> {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| LoadError(format!("external reference {reference}")))?;
        self.root
            .pointer(pointer)
            .ok_or_else(|| LoadError(format!("dangling reference {reference}")))
    }

    fn schema(&self, schema: &Value) -> Result<Schema, LoadError> {
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let target = self.resolve(reference)?;
            if target.get("enum").is_some() {
                return self.choice(target, ChoiceKind::Enum);
            }
            return self.schema(target);
        }
        if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
            let (nulls, rest): (Vec<&Value>, Vec<&Value>) = options
                .iter()
                .partition(|option| option.get("type").and_then(Value::as_str) == Some("null"));
            return match (rest.as_slice(), nulls.is_empty()) {
                ([only], true) => self.schema(only),
                ([only], false) => Ok(Schema::Nullable(Box::new(self.schema(only)?))),
                _ => Err(LoadError("anyOf with several non-null options".to_string())),
            };
        }
        if schema.get("enum").is_some() {
            return self.choice(schema, ChoiceKind::Literal);
        }
        match schema.get("type").and_then(Value::as_str) {
            None => Ok(Schema::Any),
            Some("string") => Ok(self.string(schema)),
            Some("integer") => Ok(Schema::Int(IntRules {
                ge: int_bound(schema, "minimum"),
                gt: int_bound(schema, "exclusiveMinimum"),
                le: int_bound(schema, "maximum"),
                lt: int_bound(schema, "exclusiveMaximum"),
            })),
            Some("number") => Ok(Schema::Float(FloatRules {
                ge: float_bound(schema, "minimum"),
                gt: float_bound(schema, "exclusiveMinimum"),
                le: float_bound(schema, "maximum"),
                lt: float_bound(schema, "exclusiveMaximum"),
            })),
            Some("boolean") => Ok(Schema::Bool),
            Some("array") => Ok(Schema::List {
                item: Box::new(match schema.get("items") {
                    Some(items) => self.schema(items)?,
                    None => Schema::Any,
                }),
                min_length: count(schema, "minItems"),
                max_length: count(schema, "maxItems"),
            }),
            Some("object") => self.object(schema),
            Some(other) => Err(LoadError(format!("type {other}"))),
        }
    }

    fn string(&self, schema: &Value) -> Schema {
        if schema.get("format").and_then(Value::as_str) == Some("date-time") {
            return Schema::DateTime;
        }
        Schema::Str(StrRules {
            min_length: count(schema, "minLength"),
            max_length: count(schema, "maxLength"),
            pattern: schema
                .get("pattern")
                .and_then(Value::as_str)
                .map(str::to_string),
            strip: self.options.strip_whitespace,
        })
    }

    fn choice(&self, schema: &Value, kind: ChoiceKind) -> Result<Schema, LoadError> {
        let values = schema
            .get("enum")
            .and_then(Value::as_array)
            .ok_or_else(|| LoadError("enum is not a list".to_string()))?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| LoadError("non-string enum value".to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Schema::Choice { kind, values })
    }

    fn object(&self, schema: &Value) -> Result<Schema, LoadError> {
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return Ok(Schema::Dict(match schema.get("additionalProperties") {
                Some(extra) if extra.is_object() => Some(Box::new(self.schema(extra)?)),
                _ => None,
            }));
        };
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut fields = Vec::new();
        for (name, property) in properties {
            let field_schema = self.schema(property)?;
            let default = match property.get("default") {
                Some(default) => Some(default.clone()),
                None if required.contains(&name.as_str()) => None,
                None => Some(implicit_default(&field_schema)),
            };
            fields.push(Field {
                name: name.clone(),
                schema: field_schema,
                default,
                check: None,
            });
        }
        let extra = if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            Extra::Forbid
        } else {
            Extra::Ignore
        };
        let name = schema
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Model");
        Ok(Schema::Model(Box::new(Model::new(name, extra, fields))))
    }
}

/// Pydantic leaves `default_factory` and `None` defaults out of the schema; an optional field
/// without one takes the empty value of its type.
fn implicit_default(schema: &Schema) -> Value {
    match schema {
        Schema::List { .. } => Value::Array(Vec::new()),
        Schema::Dict(_) => Value::Object(serde_json::Map::new()),
        _ => Value::Null,
    }
}
