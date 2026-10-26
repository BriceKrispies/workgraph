//! The model description validation runs against.

use serde_json::Value;

/// Whether a value must be one of a fixed set because of a `Literal` or an `Enum` annotation. The
/// two differ only in the error type (`literal_error` or `enum`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChoiceKind {
    Literal,
    Enum,
}

/// String constraints. `strip` is `str_strip_whitespace`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StrRules {
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
    pub pattern: Option<String>,
    pub strip: bool,
}

/// Integer bounds (`ge`, `gt`, `le`, `lt`). 128 bits, the width `wg-json` holds integers in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntRules {
    pub ge: Option<i128>,
    pub gt: Option<i128>,
    pub le: Option<i128>,
    pub lt: Option<i128>,
}

/// Float bounds (`ge`, `gt`, `le`, `lt`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FloatRules {
    pub ge: Option<f64>,
    pub gt: Option<f64>,
    pub le: Option<f64>,
    pub lt: Option<f64>,
}

/// The type of a value.
#[derive(Clone, Debug, PartialEq)]
pub enum Schema {
    /// `Any`: accepted unchanged.
    Any,
    Str(StrRules),
    Int(IntRules),
    Float(FloatRules),
    Bool,
    DateTime,
    /// One of a fixed set of strings.
    Choice {
        kind: ChoiceKind,
        values: Vec<String>,
    },
    List {
        item: Box<Schema>,
        min_length: Option<usize>,
        max_length: Option<usize>,
    },
    /// A `dict[str, V]`; `None` is `dict[str, Any]`.
    Dict(Option<Box<Schema>>),
    /// `T | None`.
    Nullable(Box<Schema>),
    Model(Box<Model>),
}

/// A validator Pydantic runs after a field's own type check, with the Python source's behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// `^[a-z0-9][a-z0-9-]{0,62}$`, else "slug must be lowercase letters, digits and dashes".
    Slug,
    /// Upper-cased, then `^[A-Z][A-Z0-9]{0,9}$`.
    KeyPrefix,
    /// A list of tags: each stripped, matched against `^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,99}$` and
    /// repeats dropped.
    Tags,
    /// A list of task files: repeated (path, anchor) pairs dropped.
    Files,
    /// An empty string becomes null.
    EmptyToNone,
}

/// How a model treats keys that are not fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    Forbid,
    Ignore,
}

/// A model field. `default` is `None` for a required field.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub schema: Schema,
    pub default: Option<Value>,
    pub check: Option<Check>,
}

impl Field {
    pub fn required(name: &str, schema: Schema) -> Self {
        Self {
            name: name.to_string(),
            schema,
            default: None,
            check: None,
        }
    }

    pub fn optional(name: &str, schema: Schema, default: Value) -> Self {
        Self {
            name: name.to_string(),
            schema,
            default: Some(default),
            check: None,
        }
    }

    #[must_use]
    pub fn with_check(self, check: Check) -> Self {
        Self {
            check: Some(check),
            ..self
        }
    }
}

/// A Pydantic model: a name (used in the `model_type` error), its fields in declaration order and
/// the extra-key policy.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub name: String,
    pub fields: Vec<Field>,
    pub extra: Extra,
}

impl Model {
    pub fn new(name: &str, extra: Extra, fields: Vec<Field>) -> Self {
        Self {
            name: name.to_string(),
            fields,
            extra,
        }
    }

    /// Reorders the fields to declaration order. Names not in `names` keep their relative order
    /// after the listed ones.
    #[must_use]
    pub fn ordered(self, names: &[&str]) -> Self {
        let (mut listed, rest): (Vec<Field>, Vec<Field>) = self
            .fields
            .into_iter()
            .partition(|field| names.contains(&field.name.as_str()));
        listed.sort_by_key(|field| names.iter().position(|n| *n == field.name));
        listed.extend(rest);
        Self {
            fields: listed,
            ..self
        }
    }

    /// The model with `edit` applied to the field called `name`.
    #[must_use]
    pub fn map_field(self, name: &str, edit: impl Fn(Field) -> Field) -> Self {
        Self {
            fields: self
                .fields
                .into_iter()
                .map(|field| {
                    if field.name == name {
                        edit(field)
                    } else {
                        field
                    }
                })
                .collect(),
            ..self
        }
    }
}
