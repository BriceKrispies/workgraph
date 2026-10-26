//! The three JSON dialects: the CLI `--json` output (O01), the HTTP body (O06) and the JSON text
//! stored in DB columns (O10). Each one is a conversion from `Value` to a JSON tree followed by a
//! layout pass that reproduces Python's `json.dumps` options for that dialect.

use wg_types::Timestamp;

use crate::text::{
    date_isoformat, local_datetime_str, model_str, py_bytes_repr, py_float_repr, utc_datetime_str,
};
use crate::value::{DumpError, PyStr, Value};

/// Which Python call produced the output being reproduced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialect {
    /// `json.dumps(to_jsonable(v), indent=2, default=str)` plus a newline.
    Cli,
    /// Starlette `JSONResponse(to_jsonable(v))`: compact, `ensure_ascii=False`, `allow_nan=False`,
    /// no `default=`.
    Http,
    /// `json.dumps(v, sort_keys=True, separators=(",", ":"), default=str)`.
    Db,
}

/// A JSON tree, before layout.
enum Node {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(PyStr),
    Arr(Vec<Node>),
    Obj(Vec<(PyStr, Node)>),
}

#[derive(Clone, Copy)]
struct Layout {
    pretty: bool,
    sort_keys: bool,
    ensure_ascii: bool,
    allow_nan: bool,
    /// Compact output with Python's default separators `", "` and `": "` (no indent given).
    spaced: bool,
}

/// O01: the CLI `--json` output, `json.dumps(to_jsonable(v), indent=2, default=str) + "\n"`.
pub fn cli_json(value: &Value) -> Result<String, DumpError> {
    let node = to_node(value, Dialect::Cli, false)?;
    let mut text = render(
        &node,
        Layout {
            pretty: true,
            sort_keys: false,
            ensure_ascii: true,
            allow_nan: true,
            spaced: false,
        },
    )?;
    text.push('\n');
    Ok(text)
}

/// O06: the HTTP response body, as Starlette's `JSONResponse` renders it.
pub fn http_json(value: &Value) -> Result<String, DumpError> {
    let node = to_node(value, Dialect::Http, false)?;
    render(
        &node,
        Layout {
            pretty: false,
            sort_keys: false,
            ensure_ascii: false,
            allow_nan: false,
            spaced: false,
        },
    )
}

/// O10: the text stored in a JSON column. A null value binds as SQL NULL, which is `None`.
pub fn db_json(value: &Value) -> Result<Option<String>, DumpError> {
    if matches!(value, Value::Null) {
        return Ok(None);
    }
    let node = to_node(value, Dialect::Db, false)?;
    render(
        &node,
        Layout {
            pretty: false,
            sort_keys: true,
            ensure_ascii: true,
            allow_nan: true,
            spaced: false,
        },
    )
    .map(Some)
}

/// O14: one log line's JSON, `json.dumps(payload, default=str)` (the `JsonFormatter` of
/// `core/logging.py`): Python's default separators, `ensure_ascii`, NaN allowed, keys in order.
pub fn log_json(value: &Value) -> Result<String, DumpError> {
    let node = to_node(value, Dialect::Db, false)?;
    render(
        &node,
        Layout {
            pretty: false,
            sort_keys: false,
            ensure_ascii: true,
            allow_nan: true,
            spaced: true,
        },
    )
}

/// `to_jsonable` for the CLI and HTTP dialects, and `default=str` for the CLI and DB dialects.
/// `in_model` marks values under a Pydantic model, which Pydantic serialises itself: datetimes
/// there use the `Z` form for a zero offset.
fn to_node(value: &Value, dialect: Dialect, in_model: bool) -> Result<Node, DumpError> {
    Ok(match value {
        Value::Null => Node::Null,
        Value::Bool(b) => Node::Bool(*b),
        Value::Int(i) => Node::Int(*i),
        Value::Float(f) => Node::Float(*f),
        Value::Str(s) => Node::Str(s.clone()),
        Value::List(items) => Node::Arr(
            items
                .iter()
                .map(|item| to_node(item, dialect, in_model))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Value::Dict(pairs) => Node::Obj(
            pairs
                .iter()
                .map(|(k, v)| Ok((k.clone(), to_node(v, dialect, in_model)?)))
                .collect::<Result<Vec<_>, DumpError>>()?,
        ),
        Value::DateTime(ts) => Node::Str(PyStr::from(utc_datetime(ts, dialect, in_model))),
        Value::LocalDateTime(dt) => Node::Str(PyStr::from(match dialect {
            Dialect::Db => local_datetime_str(dt),
            _ => crate::text::isoformat(dt),
        })),
        Value::Date(d) => {
            refuse_under_http(dialect, in_model)?;
            Node::Str(PyStr::from(date_isoformat(d)))
        }
        Value::Decimal(text) | Value::Uuid(text) => {
            refuse_under_http(dialect, in_model)?;
            Node::Str(PyStr::from(text.as_str()))
        }
        Value::Bytes(bytes) => {
            // Under a model, Pydantic would decode the bytes as UTF-8; that path is not modelled,
            // so it is refused rather than guessed.
            if in_model {
                return Err(DumpError::NotSerializable);
            }
            refuse_under_http(dialect, in_model)?;
            Node::Str(PyStr::from(py_bytes_repr(bytes)))
        }
        Value::Model { fields, .. } => match dialect {
            Dialect::Db => Node::Str(PyStr::from(model_str(fields))),
            Dialect::Cli | Dialect::Http => Node::Obj(
                fields
                    .iter()
                    .map(|(k, v)| Ok((PyStr::from(k.as_str()), to_node(v, dialect, true)?)))
                    .collect::<Result<Vec<_>, DumpError>>()?,
            ),
        },
    })
}

/// A UTC datetime as the dialect writes it. Pydantic's model form (`Z`) applies under a model;
/// `str(datetime)` (the DB dialect) uses a space separator. Both reuse the `Timestamp` forms.
fn utc_datetime(ts: &Timestamp, dialect: Dialect, in_model: bool) -> String {
    match dialect {
        Dialect::Db => utc_datetime_str(ts),
        _ if in_model => ts.format_z(),
        _ => ts.format_isoformat(),
    }
}

/// Starlette's encoder has no `default=`, so only JSON-native values and models pass.
fn refuse_under_http(dialect: Dialect, in_model: bool) -> Result<(), DumpError> {
    if dialect == Dialect::Http && !in_model {
        return Err(DumpError::NotSerializable);
    }
    Ok(())
}

fn render(node: &Node, layout: Layout) -> Result<String, DumpError> {
    let mut writer = Writer {
        out: String::new(),
        layout,
        saw_surrogate: false,
    };
    writer.node(node, 0)?;
    if !layout.ensure_ascii && writer.saw_surrogate {
        return Err(DumpError::Unencodable);
    }
    Ok(writer.out)
}

struct Writer {
    out: String,
    layout: Layout,
    saw_surrogate: bool,
}

impl Writer {
    fn node(&mut self, node: &Node, depth: usize) -> Result<(), DumpError> {
        match node {
            Node::Null => self.out.push_str("null"),
            Node::Bool(true) => self.out.push_str("true"),
            Node::Bool(false) => self.out.push_str("false"),
            Node::Int(i) => self.out.push_str(&i.to_string()),
            Node::Float(f) => self.float(*f)?,
            Node::Str(s) => self.string(s),
            Node::Arr(items) => {
                if items.is_empty() {
                    self.out.push_str("[]");
                    return Ok(());
                }
                self.out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.comma();
                    }
                    self.newline(depth + 1);
                    self.node(item, depth + 1)?;
                }
                self.newline(depth);
                self.out.push(']');
            }
            Node::Obj(pairs) => {
                if pairs.is_empty() {
                    self.out.push_str("{}");
                    return Ok(());
                }
                let mut ordered: Vec<&(PyStr, Node)> = pairs.iter().collect();
                if self.layout.sort_keys {
                    ordered.sort_by(|a, b| a.0.cmp(&b.0));
                }
                self.out.push('{');
                for (i, pair) in ordered.into_iter().enumerate() {
                    if i > 0 {
                        self.comma();
                    }
                    self.newline(depth + 1);
                    self.string(&pair.0);
                    self.out.push(':');
                    if self.layout.pretty || self.layout.spaced {
                        self.out.push(' ');
                    }
                    self.node(&pair.1, depth + 1)?;
                }
                self.newline(depth);
                self.out.push('}');
            }
        }
        Ok(())
    }

    fn comma(&mut self) {
        self.out.push(',');
        if self.layout.spaced {
            self.out.push(' ');
        }
    }

    fn newline(&mut self, depth: usize) {
        if self.layout.pretty {
            self.out.push('\n');
            for _ in 0..depth {
                self.out.push_str("  ");
            }
        }
    }

    fn float(&mut self, value: f64) -> Result<(), DumpError> {
        if value.is_finite() {
            self.out.push_str(&py_float_repr(value));
            return Ok(());
        }
        if !self.layout.allow_nan {
            return Err(DumpError::NonFiniteFloat);
        }
        self.out.push_str(if value.is_nan() {
            "NaN"
        } else if value > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        });
        Ok(())
    }

    /// Python's `encode_basestring` (or `encode_basestring_ascii` when `ensure_ascii`).
    fn string(&mut self, text: &PyStr) {
        self.out.push('"');
        for &cp in text.code_points() {
            match cp {
                0x22 => self.out.push_str("\\\""),
                0x5c => self.out.push_str("\\\\"),
                0x08 => self.out.push_str("\\b"),
                0x0c => self.out.push_str("\\f"),
                0x0a => self.out.push_str("\\n"),
                0x0d => self.out.push_str("\\r"),
                0x09 => self.out.push_str("\\t"),
                0x00..=0x1f => self.out.push_str(&format!("\\u{cp:04x}")),
                0x20..=0x7e => self.push_char(cp),
                0x7f if self.layout.ensure_ascii => self.out.push_str("\\u007f"),
                0x7f => self.push_char(cp),
                0xd800..=0xdfff => {
                    self.saw_surrogate = true;
                    if self.layout.ensure_ascii {
                        self.out.push_str(&format!("\\u{cp:04x}"));
                    }
                }
                _ if !self.layout.ensure_ascii => self.push_char(cp),
                0x10000.. => {
                    let v = cp - 0x10000;
                    let high = 0xd800 + (v >> 10);
                    let low = 0xdc00 + (v & 0x3ff);
                    self.out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                }
                _ => self.out.push_str(&format!("\\u{cp:04x}")),
            }
        }
        self.out.push('"');
    }

    fn push_char(&mut self, cp: u32) {
        if let Some(c) = char::from_u32(cp) {
            self.out.push(c);
        }
    }
}
