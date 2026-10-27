//! Python semantics for untyped JSON values (`serde_json::Value`).
//!
//! Code that ports Python reads decoded JSON the way Python does: `d.get("k")`, `d.get("k") or
//! default`, `value[:n]`, `if value:`. These functions are those idioms, one call each, on the
//! member a lookup returns, so the same call serves a `Value` and a `Map<String, Value>`.
//!
//! ```
//! use serde_json::json;
//! use wg_types::pyval::{flag, get, head, int_or, or_value, prefix, text, text_or, truthy};
//!
//! let row = json!({"name": "build", "tries": 3, "done": true, "note": ""});
//! assert_eq!(text(row.get("name")), "build"); // row.get("name", "")
//! assert_eq!(text_or(row.get("note"), "none"), "none"); // row.get("note") or "none"
//! assert_eq!(int_or(row.get("tries"), 0), 3); // row.get("tries", 0)
//! assert!(flag(row.get("done"))); // row.get("done") is True
//! assert!(get(&row, "missing").is_null()); // row.get("missing")
//! assert!(!truthy(get(&row, "note"))); // bool("")
//! assert_eq!(or_value(row.get("note"), json!([])), json!([])); // row.get("note") or []
//! assert_eq!(prefix("héllo", 2), "hé"); // "héllo"[:2]
//! assert_eq!(head(&[1, 2, 3], 2), &[1, 2]); // [1, 2, 3][:2]
//! ```
//!
//! Objects are built with `serde_json::json!` (key order is kept: `preserve_order`); the dump-side
//! `wg_json::Value` has its own `dict!` macro.

use serde_json::{Map, Value};

/// Python truthiness (`bool(value)`): `None`, `False`, zero, `""`, `[]` and `{}` are falsy.
///
/// A number that has no `f64` form is never zero.
#[must_use]
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_none_or(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(members) => !members.is_empty(),
    }
}

/// [`truthy`] of a member that may be absent: an absent member is falsy.
#[must_use]
pub fn truthy_opt(value: Option<&Value>) -> bool {
    value.is_some_and(truthy)
}

/// `value.get(key)` with `None` standing for `null`: the member, or `Null`. A non-object gives
/// `Null`.
#[must_use]
pub fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

/// `d.get(k, "")` for a text member: the string, or `""` when the member is absent or is not a
/// string. The argument is the lookup, `text(d.get("k"))`, so it serves a `Value` and a `Map`.
#[must_use]
pub fn text(member: Option<&Value>) -> &str {
    member.and_then(Value::as_str).unwrap_or_default()
}

/// `d.get(k) or default` for a text member: the string unless it is absent, not a string or empty.
#[must_use]
pub fn text_or<'a>(member: Option<&'a Value>, default: &'a str) -> &'a str {
    nonempty(member.and_then(Value::as_str)).unwrap_or(default)
}

/// `text or None`: an empty string is treated as absent.
#[must_use]
pub fn nonempty(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.is_empty())
}

/// `d.get(k, default)` for an integer member: the integer, or `default` when the member is absent
/// or is not an integer.
#[must_use]
pub fn int_or(member: Option<&Value>, default: i64) -> i64 {
    member.and_then(Value::as_i64).unwrap_or(default)
}

/// `d.get(k) is True` for a flag member: `true` only for a JSON `true`.
#[must_use]
pub fn flag(member: Option<&Value>) -> bool {
    member.and_then(Value::as_bool).unwrap_or(false)
}

/// `d.get(k)` for an object member: a copy of the object, or an empty one when the member is absent
/// or is not an object.
#[must_use]
pub fn object(member: Option<&Value>) -> Map<String, Value> {
    member
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `value or default`: the member when it is present and truthy, else `default`.
#[must_use]
pub fn or_value(value: Option<&Value>, default: Value) -> Value {
    match value {
        Some(found) if truthy(found) => found.clone(),
        _ => default,
    }
}

/// `items[:n]`.
#[must_use]
pub fn head<T>(items: &[T], n: usize) -> &[T] {
    &items[..n.min(items.len())]
}

/// `text[:n]`, counted in characters (code points), never splitting one.
#[must_use]
pub fn prefix(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truthiness_follows_python() {
        let falsy = [
            json!(null),
            json!(false),
            json!(0),
            json!(0.0),
            json!(""),
            json!([]),
            json!({}),
        ];
        let truthy_values = [
            json!(true),
            json!(1),
            json!(-0.5),
            json!("x"),
            json!([0]),
            json!({"a": 0}),
        ];
        assert!(falsy.iter().all(|v| !truthy(v)));
        assert!(truthy_values.iter().all(truthy));
        assert!(!truthy_opt(None));
        assert!(truthy_opt(Some(&json!(2))));
    }

    #[test]
    fn members_of_values_and_maps() {
        let value = json!({"a": "x", "b": "", "n": 7, "t": true, "f": 1});
        let map = value.as_object().cloned().unwrap_or_default();
        assert_eq!(text(value.get("a")), "x");
        assert_eq!(text(map.get("a")), "x");
        assert_eq!(text(value.get("n")), "");
        assert_eq!(text(json!([1]).get("a")), "");
        assert_eq!(text_or(value.get("b"), "d"), "d");
        assert_eq!(text_or(map.get("a"), "d"), "x");
        assert_eq!(int_or(value.get("n"), 0), 7);
        assert_eq!(int_or(value.get("a"), 4), 4);
        assert!(flag(map.get("t")));
        assert!(!flag(map.get("f")));
        assert!(get(&value, "zz").is_null());
        assert_eq!(get(&value, "n"), &json!(7));
        assert_eq!(nonempty(Some("")), None);
        assert_eq!(nonempty(Some("a")), Some("a"));
    }

    #[test]
    fn defaults_and_slices() {
        assert_eq!(or_value(None, json!({})), json!({}));
        assert_eq!(object(json!({"a": {"b": 1}}).get("a")).len(), 1);
        assert!(object(json!({"a": 1}).get("a")).is_empty());
        assert!(object(None).is_empty());
        assert_eq!(or_value(Some(&json!(0)), json!(9)), json!(9));
        assert_eq!(or_value(Some(&json!([1])), json!(9)), json!([1]));
        assert_eq!(head(&[1, 2, 3], 2), &[1, 2]);
        assert_eq!(head(&[1], 5), &[1]);
        assert_eq!(prefix("héllo", 2), "hé");
        assert_eq!(prefix("ab", 9), "ab");
    }
}
