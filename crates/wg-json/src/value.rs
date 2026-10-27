//! The values the three JSON dialects accept, as the Python side sees them.

use std::fmt;

use wg_types::Timestamp;

/// A Python `str`: a sequence of code points. Unlike a Rust `String` it may hold lone surrogates,
/// which `ensure_ascii` output escapes and UTF-8 output refuses.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PyStr(Vec<u32>);

impl PyStr {
    /// The code points of the string.
    pub fn code_points(&self) -> &[u32] {
        &self.0
    }

    /// Builds a string from code points, which may include surrogates.
    pub fn from_code_points(code_points: Vec<u32>) -> Self {
        Self(code_points)
    }
}

impl From<&str> for PyStr {
    fn from(text: &str) -> Self {
        Self(text.chars().map(u32::from).collect())
    }
}

impl From<String> for PyStr {
    fn from(text: String) -> Self {
        Self::from(text.as_str())
    }
}

/// A `datetime.datetime` that `wg_types::Timestamp` cannot hold: a naive value (`offset_seconds`
/// `None`) or one with a non-zero fixed UTC offset in seconds. A UTC value is a `Timestamp` and
/// belongs in [`Value::DateTime`]; a zero offset must not be stored here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalDateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub microsecond: u32,
    pub offset_seconds: Option<i32>,
}

/// A `datetime.date`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

/// A value that one of the dialects can serialise.
///
/// Python `tuple` and `set` values are represented as `List`, because `to_jsonable` turns both
/// into a JSON array. `Decimal` and `UUID` are held as their `str()` text; the caller supplies the
/// canonical form. Integers are held in 128 bits.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(PyStr),
    List(Vec<Value>),
    Dict(Vec<(PyStr, Value)>),
    /// A UTC datetime, held by the shared `wg-types` timestamp type.
    DateTime(Timestamp),
    /// A naive or non-UTC wall-clock datetime; see [`LocalDateTime`].
    LocalDateTime(LocalDateTime),
    Date(Date),
    Decimal(String),
    Uuid(String),
    Bytes(Vec<u8>),
    /// A Pydantic model: class name and field values in declaration order.
    Model {
        name: String,
        fields: Vec<(String, Value)>,
    },
}

/// A Python dict literal as a [`Value::Dict`]: the keys in the order written, written like
/// `serde_json::json!` writes an object.
///
/// ```
/// use wg_json::{Value, dict};
///
/// let row = dict!({
///     "name": Value::Str("build".into()),
///     "tries": Value::Int(3),
///     "tags": Value::List(vec![]),
/// });
/// assert!(matches!(row, Value::Dict(ref entries) if entries.len() == 3));
/// ```
#[macro_export]
macro_rules! dict {
    ({ $($key:literal : $value:expr),* $(,)? }) => {
        $crate::dict(vec![$(($key, $value)),*])
    };
}

/// A [`Value::Dict`] from fields built at run time, keys in the order given. A fixed set of keys
/// reads better as [`dict!`].
pub fn dict(fields: Vec<(&str, Value)>) -> Value {
    Value::Dict(
        fields
            .into_iter()
            .map(|(key, value)| (PyStr::from(key), value))
            .collect(),
    )
}

/// The Python exception a dialect raises when a value cannot be serialised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DumpError {
    /// `ValueError` from `allow_nan=False` meeting a NaN or infinity.
    NonFiniteFloat,
    /// `TypeError` from a type the dialect has no `default=` for.
    NotSerializable,
    /// `UnicodeEncodeError` from encoding a lone surrogate as UTF-8.
    Unencodable,
}

impl DumpError {
    /// The Python exception class name.
    pub fn python_name(self) -> &'static str {
        match self {
            Self::NonFiniteFloat => "ValueError",
            Self::NotSerializable => "TypeError",
            Self::Unencodable => "UnicodeEncodeError",
        }
    }
}

impl fmt::Display for DumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.python_name())
    }
}
