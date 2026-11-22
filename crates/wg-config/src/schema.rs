//! The shape of a configuration model: Pydantic's models, their field types and their defaults.
//!
//! A [`Model`] is one `BaseModel` of `settings.py`: its fields in declaration order (which is the
//! order of the dump), each with a [`Schema`] and an optional default. A default is an input that
//! is validated like any other, so a default factory such as `ServerConfig` is written as `{}`.

use serde_json::Value;

/// The type of a field, with the constraints Pydantic checks for it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Schema {
    /// `Any`: accepted unchanged.
    Any,
    /// `str`.
    Str,
    /// `str` whose stripped value must not be empty. `message` is the text of the `ValueError`.
    NonBlank { message: &'static str },
    /// `int` with an optional lower and upper bound (inclusive).
    Int { ge: Option<i64>, le: Option<i64> },
    /// `float` with an optional lower bound (inclusive) and exclusive lower bound.
    Float { ge: Option<f64>, gt: Option<f64> },
    /// `bool`.
    Bool,
    /// `Literal[...]`: one of a fixed set of strings.
    Literal(&'static [&'static str]),
    /// `T | None`.
    Nullable(Box<Schema>),
    /// `list[T]` with a minimum length.
    List { item: Box<Schema>, min_len: usize },
    /// `list[str]` whose every entry must be an IPv4 or IPv6 network.
    Networks,
    /// `list[str]` of at least one Jira project key.
    JiraProjects,
    /// `dict[str, V]`.
    Dict(Box<Schema>),
    /// A nested model.
    Model(Model),
    /// A discriminated union on the `type` key, one model per tag.
    Tagged(Vec<(&'static str, Model)>),
    /// `inner`, then a Pydantic `after` validator over its dump: `Some(message)` is a `ValueError`.
    Checked(Box<Schema>, Check),
}

/// The function of a [`Schema::Checked`] validator.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Check(pub fn(&Value) -> Option<String>);

impl PartialEq for Check {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::fn_addr_eq(self.0, other.0)
    }
}

/// One field of a model.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Field {
    pub name: &'static str,
    pub schema: Schema,
    /// The input used when the key is absent, or `None` for a required field.
    pub default: Option<Value>,
}

/// A Pydantic model with `extra="forbid"`: its fields in declaration order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Model {
    pub name: &'static str,
    pub fields: Vec<Field>,
}
