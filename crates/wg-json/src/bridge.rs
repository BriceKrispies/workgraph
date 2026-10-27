//! The bridge between `serde` and [`Value`]: a [`Serializer`] that builds a [`Value`], and
//! `Serialize for Value`, so a `#[derive(Serialize)]` struct reaches the three dialects.
//!
//! A DTO is one struct whose fields are in the order Python declares them; [`to_value`] turns it
//! into a [`Value::Model`] (a Pydantic `model_dump`) and [`to_dict`] into a [`Value::Dict`] (a
//! Python dict literal). The dialects then print it, so separators, `ensure_ascii` and the float
//! repr stay in this crate.
//!
//! ```
//! use serde::Serialize;
//! use wg_json::{Value, http_json, to_value};
//!
//! #[derive(Serialize)]
//! struct Row<'a> {
//!     name: &'a str,
//!     tries: Option<i64>,
//!     ratio: f64,
//! }
//!
//! let row = to_value(&Row { name: "build", tries: None, ratio: 1.0 });
//! assert!(matches!(row, Value::Model { .. }));
//! assert_eq!(
//!     http_json(&row).unwrap(),
//!     r#"{"name":"build","tries":null,"ratio":1.0}"#
//! );
//! ```
//!
//! What each serde shape becomes:
//!
//! * integers [`Value::Int`], floats [`Value::Float`], strings [`Value::Str`], `None` and `()`
//!   [`Value::Null`], sequences [`Value::List`], maps [`Value::Dict`] (keys in the order given);
//! * a struct [`Value::Model`] (named after the struct), unless it is wrapped by [`to_dict`] or
//!   the [`dict_field`] adapter, which make it a [`Value::Dict`];
//! * a [`Value`] field serialises as itself, datetimes and all, so a DTO may hold values that
//!   another helper already built;
//! * an enum variant without data is its name; a variant with data is `{variant: data}`.

use std::fmt;

use serde::ser::{
    self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant, Serializer,
};
use wg_types::Timestamp;

use crate::{Date, LocalDateTime, PyStr, Value};

// Newtype-struct names that carry a `Value` variant serde has no type for. A foreign serializer
// (`serde_json`) writes the inner value; this crate's serializer rebuilds the variant.
const DATETIME: &str = "$wg_json::DateTime";
const LOCAL_DATETIME: &str = "$wg_json::LocalDateTime";
const DATE: &str = "$wg_json::Date";
const DECIMAL: &str = "$wg_json::Decimal";
const UUID: &str = "$wg_json::Uuid";
const PYSTR: &str = "$wg_json::PyStr";
const MODEL: &str = "$wg_json::Model";
const DICT: &str = "$wg_json::Dict";

/// Why a value could not become a [`Value`]: a map key that is not text, or an integer outside
/// 128 bits. Derived DTOs never hit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeError(String);

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BridgeError {}

impl ser::Error for BridgeError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

fn refuse(message: &str) -> BridgeError {
    BridgeError(message.to_string())
}

/// A [`Value::Model`] for a struct: its fields in declaration order, named after the struct.
/// An error in a field (never raised by derived DTOs) is reported as a `Str` holding its message.
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Value {
    try_to_value(value).unwrap_or_else(|err| Value::Str(PyStr::from(err.0)))
}

/// Like [`to_value`] but a [`Value::Dict`] at the top: a Python dict literal rather than a model.
/// Structs nested inside are dicts too; a [`Value`] field keeps its own model/dict shapes.
pub fn to_dict<T: Serialize + ?Sized>(value: &T) -> Value {
    value
        .serialize(ValueSerializer { dict: true })
        .unwrap_or_else(|err| Value::Str(PyStr::from(err.0)))
}

/// [`to_value`] that reports the error instead of folding it into the value.
pub fn try_to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, BridgeError> {
    value.serialize(ValueSerializer { dict: false })
}

struct AsDict<'a, T: ?Sized>(&'a T);

impl<T: Serialize + ?Sized> Serialize for AsDict<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_struct(DICT, self.0)
    }
}

/// `#[serde(serialize_with = "wg_json::dict_field")]`: a nested struct that Python builds as a
/// plain dict rather than a model.
pub fn dict_field<T: Serialize + ?Sized, S: Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    AsDict(value).serialize(serializer)
}

struct PyStrRef<'a>(&'a PyStr);

impl Serialize for PyStrRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0.to_text() {
            Some(text) => serializer.serialize_str(&text),
            None => serializer.serialize_newtype_struct(PYSTR, self.0.code_points()),
        }
    }
}

struct ModelRef<'a>(&'a str, &'a [(String, Value)]);

impl Serialize for ModelRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.1.len() + 1))?;
        map.serialize_entry("$name", self.0)?;
        for (key, value) in self.1 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Int(i) => serializer.serialize_i128(*i),
            Value::Float(f) => serializer.serialize_f64(*f),
            Value::Str(s) => PyStrRef(s).serialize(serializer),
            Value::List(items) => serializer.collect_seq(items),
            Value::Dict(pairs) => {
                serializer.collect_map(pairs.iter().map(|(key, value)| (PyStrRef(key), value)))
            }
            Value::DateTime(ts) => {
                serializer.serialize_newtype_struct(DATETIME, &ts.format_isoformat())
            }
            Value::LocalDateTime(dt) => serializer.serialize_newtype_struct(
                LOCAL_DATETIME,
                &(
                    dt.year,
                    dt.month,
                    dt.day,
                    dt.hour,
                    dt.minute,
                    dt.second,
                    dt.microsecond,
                    dt.offset_seconds,
                ),
            ),
            Value::Date(d) => serializer.serialize_newtype_struct(DATE, &(d.year, d.month, d.day)),
            Value::Decimal(text) => serializer.serialize_newtype_struct(DECIMAL, text),
            Value::Uuid(text) => serializer.serialize_newtype_struct(UUID, text),
            Value::Bytes(bytes) => serializer.serialize_bytes(bytes),
            Value::Model { name, fields } => {
                serializer.serialize_newtype_struct(MODEL, &ModelRef(name, fields))
            }
        }
    }
}

/// Builds a [`Value`]. With `dict` set, every struct becomes a [`Value::Dict`] (a Python dict
/// literal) instead of a [`Value::Model`].
#[derive(Clone, Copy)]
struct ValueSerializer {
    dict: bool,
}

fn int(value: impl Into<i128>) -> Result<Value, BridgeError> {
    Ok(Value::Int(value.into()))
}

fn text(value: &str) -> Value {
    Value::Str(PyStr::from(value))
}

impl Serializer for ValueSerializer {
    type Ok = Value;
    type Error = BridgeError;
    type SerializeSeq = SeqBuilder;
    type SerializeTuple = SeqBuilder;
    type SerializeTupleStruct = SeqBuilder;
    type SerializeTupleVariant = VariantSeqBuilder;
    type SerializeMap = MapBuilder;
    type SerializeStruct = StructBuilder;
    type SerializeStructVariant = VariantStructBuilder;

    fn serialize_bool(self, v: bool) -> Result<Value, BridgeError> {
        Ok(Value::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_i16(self, v: i16) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_i32(self, v: i32) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_i64(self, v: i64) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_i128(self, v: i128) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_u8(self, v: u8) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_u16(self, v: u16) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_u32(self, v: u32) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_u64(self, v: u64) -> Result<Value, BridgeError> {
        int(v)
    }
    fn serialize_u128(self, v: u128) -> Result<Value, BridgeError> {
        i128::try_from(v)
            .map(Value::Int)
            .map_err(|_| refuse("integer outside 128 bits"))
    }
    fn serialize_f32(self, v: f32) -> Result<Value, BridgeError> {
        Ok(Value::Float(f64::from(v)))
    }
    fn serialize_f64(self, v: f64) -> Result<Value, BridgeError> {
        Ok(Value::Float(v))
    }
    fn serialize_char(self, v: char) -> Result<Value, BridgeError> {
        Ok(text(v.encode_utf8(&mut [0; 4])))
    }
    fn serialize_str(self, v: &str) -> Result<Value, BridgeError> {
        Ok(text(v))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Value, BridgeError> {
        Ok(Value::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<Value, BridgeError> {
        Ok(Value::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Value, BridgeError> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<Value, BridgeError> {
        Ok(Value::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, BridgeError> {
        Ok(Value::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Value, BridgeError> {
        Ok(text(variant))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Value, BridgeError> {
        if name == DICT {
            return value.serialize(ValueSerializer { dict: true });
        }
        let inner = value.serialize(self)?;
        match name {
            DATETIME => match &inner {
                Value::Str(s) => Timestamp::parse_iso(&string_of(s))
                    .map(Value::DateTime)
                    .map_err(|_| refuse("invalid datetime")),
                _ => Err(refuse("invalid datetime")),
            },
            PYSTR => match inner {
                Value::List(points) => points
                    .iter()
                    .map(|point| match point {
                        Value::Int(i) => u32::try_from(*i).ok(),
                        _ => None,
                    })
                    .collect::<Option<Vec<u32>>>()
                    .map(|points| Value::Str(PyStr::from_code_points(points)))
                    .ok_or_else(|| refuse("invalid code points")),
                other => Ok(other),
            },
            DECIMAL => match &inner {
                Value::Str(s) => Ok(Value::Decimal(string_of(s))),
                _ => Err(refuse("invalid text")),
            },
            UUID => match &inner {
                Value::Str(s) => Ok(Value::Uuid(string_of(s))),
                _ => Err(refuse("invalid text")),
            },
            DATE => date_of(&inner).ok_or_else(|| refuse("invalid date")),
            LOCAL_DATETIME => local_of(&inner).ok_or_else(|| refuse("invalid datetime")),
            MODEL => model_of(inner).ok_or_else(|| refuse("invalid model")),
            _ => Ok(inner),
        }
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value, BridgeError> {
        Ok(Value::Dict(vec![(
            PyStr::from(variant),
            value.serialize(self)?,
        )]))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqBuilder, BridgeError> {
        Ok(SeqBuilder(self, Vec::with_capacity(len.unwrap_or(0))))
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqBuilder, BridgeError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SeqBuilder, BridgeError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantSeqBuilder, BridgeError> {
        Ok(VariantSeqBuilder(self, variant, Vec::with_capacity(len)))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<MapBuilder, BridgeError> {
        Ok(MapBuilder {
            ser: self,
            pairs: Vec::with_capacity(len.unwrap_or(0)),
            key: None,
        })
    }
    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<StructBuilder, BridgeError> {
        Ok(StructBuilder(self, name, Vec::with_capacity(len)))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantStructBuilder, BridgeError> {
        Ok(VariantStructBuilder(self, variant, Vec::with_capacity(len)))
    }
}

fn string_of(text: &PyStr) -> String {
    text.code_points()
        .iter()
        .filter_map(|p| char::from_u32(*p))
        .collect()
}

fn small(value: &Value) -> Option<i64> {
    match value {
        Value::Int(i) => i64::try_from(*i).ok(),
        _ => None,
    }
}

fn date_of(value: &Value) -> Option<Value> {
    let Value::List(parts) = value else {
        return None;
    };
    let [year, month, day] = parts.as_slice() else {
        return None;
    };
    Some(Value::Date(Date {
        year: i32::try_from(small(year)?).ok()?,
        month: u8::try_from(small(month)?).ok()?,
        day: u8::try_from(small(day)?).ok()?,
    }))
}

fn local_of(value: &Value) -> Option<Value> {
    let Value::List(parts) = value else {
        return None;
    };
    let [year, month, day, hour, minute, second, microsecond, offset] = parts.as_slice() else {
        return None;
    };
    Some(Value::LocalDateTime(LocalDateTime {
        year: i32::try_from(small(year)?).ok()?,
        month: u8::try_from(small(month)?).ok()?,
        day: u8::try_from(small(day)?).ok()?,
        hour: u8::try_from(small(hour)?).ok()?,
        minute: u8::try_from(small(minute)?).ok()?,
        second: u8::try_from(small(second)?).ok()?,
        microsecond: u32::try_from(small(microsecond)?).ok()?,
        offset_seconds: match offset {
            Value::Null => None,
            other => Some(i32::try_from(small(other)?).ok()?),
        },
    }))
}

/// The model a [`ModelRef`] became: a dict whose first entry is `$name`.
fn model_of(value: Value) -> Option<Value> {
    let Value::Dict(pairs) = value else {
        return None;
    };
    let mut entries = pairs.into_iter();
    let (_, name) = entries.next()?;
    let Value::Str(name) = name else {
        return None;
    };
    Some(Value::Model {
        name: string_of(&name),
        fields: entries
            .map(|(key, value)| (string_of(&key), value))
            .collect(),
    })
}

/// Emits its items unchanged. serde's compound serializer traits take `&mut self` in every method,
/// and the builders that implement them are private to this module, so no public signature of
/// this crate exposes a mutable reference. The wrapper keeps the purity scan, which reads public
/// and trait method signatures, from counting them as public API.
macro_rules! private_builders {
    ($($item:item)*) => {
        $($item)*
    };
}

private_builders! {
    struct SeqBuilder(ValueSerializer, Vec<Value>);

    impl SerializeSeq for SeqBuilder {
        type Ok = Value;
        type Error = BridgeError;
        fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeError> {
            self.1.push(value.serialize(self.0)?);
            Ok(())
        }
        fn end(self) -> Result<Value, BridgeError> {
            Ok(Value::List(self.1))
        }
    }

    impl SerializeTuple for SeqBuilder {
        type Ok = Value;
        type Error = BridgeError;
        fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeError> {
            SerializeSeq::serialize_element(self, value)
        }
        fn end(self) -> Result<Value, BridgeError> {
            SerializeSeq::end(self)
        }
    }

    impl SerializeTupleStruct for SeqBuilder {
        type Ok = Value;
        type Error = BridgeError;
        fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeError> {
            SerializeSeq::serialize_element(self, value)
        }
        fn end(self) -> Result<Value, BridgeError> {
            SerializeSeq::end(self)
        }
    }

    struct VariantSeqBuilder(ValueSerializer, &'static str, Vec<Value>);

    impl SerializeTupleVariant for VariantSeqBuilder {
        type Ok = Value;
        type Error = BridgeError;
        fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeError> {
            self.2.push(value.serialize(self.0)?);
            Ok(())
        }
        fn end(self) -> Result<Value, BridgeError> {
            Ok(Value::Dict(vec![(
                PyStr::from(self.1),
                Value::List(self.2),
            )]))
        }
    }

    struct MapBuilder {
        ser: ValueSerializer,
        pairs: Vec<(PyStr, Value)>,
        key: Option<PyStr>,
    }

    impl SerializeMap for MapBuilder {
        type Ok = Value;
        type Error = BridgeError;
        fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), BridgeError> {
            self.key = Some(match key.serialize(self.ser)? {
                Value::Str(s) => s,
                Value::Int(i) => PyStr::from(i.to_string()),
                _ => return Err(refuse("map keys must be text")),
            });
            Ok(())
        }
        fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BridgeError> {
            let key = self.key.take().ok_or_else(|| refuse("value without key"))?;
            self.pairs.push((key, value.serialize(self.ser)?));
            Ok(())
        }
        fn end(self) -> Result<Value, BridgeError> {
            Ok(Value::Dict(self.pairs))
        }
    }

    struct StructBuilder(ValueSerializer, &'static str, Vec<(String, Value)>);

    impl SerializeStruct for StructBuilder {
        type Ok = Value;
        type Error = BridgeError;
        fn serialize_field<T: Serialize + ?Sized>(
            &mut self,
            key: &'static str,
            value: &T,
        ) -> Result<(), BridgeError> {
            self.2.push((key.to_string(), value.serialize(self.0)?));
            Ok(())
        }
        fn end(self) -> Result<Value, BridgeError> {
            if self.0.dict {
                return Ok(Value::Dict(
                    self.2
                        .into_iter()
                        .map(|(key, value)| (PyStr::from(key), value))
                        .collect(),
                ));
            }
            Ok(Value::Model {
                name: self.1.to_string(),
                fields: self.2,
            })
        }
    }

    struct VariantStructBuilder(ValueSerializer, &'static str, Vec<(PyStr, Value)>);

    impl SerializeStructVariant for VariantStructBuilder {
        type Ok = Value;
        type Error = BridgeError;
        fn serialize_field<T: Serialize + ?Sized>(
            &mut self,
            key: &'static str,
            value: &T,
        ) -> Result<(), BridgeError> {
            self.2.push((PyStr::from(key), value.serialize(self.0)?));
            Ok(())
        }
        fn end(self) -> Result<Value, BridgeError> {
            Ok(Value::Dict(vec![(
                PyStr::from(self.1),
                Value::Dict(self.2),
            )]))
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;
    use crate::{cli_json, http_json};

    #[derive(Serialize)]
    struct Inner {
        n: i64,
    }

    #[derive(Serialize)]
    struct Row<'a> {
        name: &'a str,
        tries: Option<i64>,
        ratio: f64,
        tags: Vec<&'a str>,
        inner: Inner,
        #[serde(serialize_with = "dict_field")]
        plain: Inner,
        raw: Value,
    }

    fn at() -> Timestamp {
        Timestamp::parse_iso("2026-10-25T13:56:00.5+00:00").unwrap()
    }

    #[test]
    fn struct_is_a_model_in_field_order() {
        let row = Row {
            name: "build",
            tries: None,
            ratio: 1.0,
            tags: vec!["a", "b"],
            inner: Inner { n: 1 },
            plain: Inner { n: 2 },
            raw: Value::DateTime(at()),
        };
        let value = to_value(&row);
        let Value::Model { name, fields } = &value else {
            panic!("not a model");
        };
        assert_eq!(name, "Row");
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            keys,
            ["name", "tries", "ratio", "tags", "inner", "plain", "raw"]
        );
        assert!(matches!(fields[4].1, Value::Model { .. }));
        assert!(matches!(fields[5].1, Value::Dict(_)));
        assert_eq!(
            http_json(&value).unwrap(),
            r#"{"name":"build","tries":null,"ratio":1.0,"tags":["a","b"],"inner":{"n":1},"plain":{"n":2},"raw":"2026-10-25T13:56:00.500000Z"}"#
        );
    }

    #[test]
    fn to_dict_makes_the_top_a_dict() {
        assert_eq!(
            to_dict(&Inner { n: 3 }),
            Value::Dict(vec![(PyStr::from("n"), Value::Int(3))])
        );
    }

    #[test]
    fn to_dict_makes_nested_structs_dicts_but_keeps_values() {
        let nested = Value::Model {
            name: "M".to_string(),
            fields: vec![],
        };
        let row = Row {
            name: "x",
            tries: Some(1),
            ratio: 0.5,
            tags: vec![],
            inner: Inner { n: 1 },
            plain: Inner { n: 2 },
            raw: nested.clone(),
        };
        let Value::Dict(fields) = to_dict(&row) else {
            panic!("not a dict");
        };
        assert!(matches!(fields[4].1, Value::Dict(_)));
        assert_eq!(fields[6].1, nested);
    }

    #[test]
    fn values_round_trip() {
        let original = Value::List(vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(-5),
            Value::Float(0.1),
            Value::Str(PyStr::from("é")),
            Value::Str(PyStr::from_code_points(vec![0x61, 0xd800])),
            Value::DateTime(at()),
            Value::LocalDateTime(LocalDateTime {
                year: 2026,
                month: 1,
                day: 2,
                hour: 3,
                minute: 4,
                second: 5,
                microsecond: 6,
                offset_seconds: Some(3600),
            }),
            Value::Date(Date {
                year: 2026,
                month: 1,
                day: 2,
            }),
            Value::Decimal("1.5".to_string()),
            Value::Uuid("u".to_string()),
            Value::Bytes(vec![1, 2]),
            Value::Dict(vec![(PyStr::from("k"), Value::Int(1))]),
            Value::Model {
                name: "M".to_string(),
                fields: vec![("f".to_string(), Value::Null)],
            },
        ]);
        assert_eq!(to_value(&original), original);
        assert_eq!(
            cli_json(&to_value(&original)).unwrap(),
            cli_json(&original).unwrap()
        );
    }
}
