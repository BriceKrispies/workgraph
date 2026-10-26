//! The canonical database dump (CR-06): the Rust side of `tools/contracts/dbdump.py dump`.
//!
//! The output is byte for byte the output of `dbdump.py dump DB` for the same file. The steps
//! mirror the Python code one for one:
//!
//! * `sqlite_master` in `(type, name)` order, with each table's `CREATE TABLE` put in the form
//!   `dump._canonical_create_table` gives it, and each other statement stripped of trailing
//!   whitespace and semicolons.
//! * Every table except `alembic_version` and the FTS shadow tables, rows in primary-key order
//!   (or `rowid`), columns in declaration order.
//! * Text values that start with `{` or `[` are parsed as JSON, as Python's `json.loads` parses
//!   them (so `NaN` and `Infinity` are accepted), and re-serialised. Other text is kept. A BLOB is
//!   `{"blob": <hex>}`.
//! * The whole tree is written as `json.dumps(doc, sort_keys=True, indent=1, ensure_ascii=False)`
//!   plus a newline.
//!
//! The database is opened read-only. Python opens it read-write and would create a missing file,
//! so a missing path is an error here and an empty dump there.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use wg_json::py_float_repr;

/// The tables `dbdump.py` leaves out: the migration bookkeeping table and the FTS5 table with its
/// shadow tables.
const EXCLUDED_TABLES: [&str; 1] = ["alembic_version"];
const EXCLUDED_PREFIXES: [&str; 1] = ["task_fts"];

/// A JSON value as Python's `json` module sees it.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    /// Decimal digits, as `str(int)` prints them.
    Int(String),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

/// The canonical dump of `database`, as the bytes `dbdump.py dump` writes.
pub fn canonical_dump(database: &Path) -> Result<Vec<u8>, Box<dyn Error>> {
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let doc = canonical_document(&conn)?;
    let mut text = String::new();
    write_value(&doc, 0, &mut text);
    text.push('\n');
    Ok(text.into_bytes())
}

/// `dbdump._canonical_create_table` and the `_normalised_sql` rule for other statements.
pub fn canonical_create_table(sql: &str) -> String {
    let Some((head, rest)) = sql.split_once('(') else {
        return sql.to_owned();
    };
    let Some(close) = rest.rfind(')') else {
        return sql.to_owned();
    };
    let body = &rest[..close];
    let tail = &rest[close + 1..];
    let mut columns = Vec::new();
    let mut constraints = Vec::new();
    for item in split_top_level(body) {
        if is_constraint_start(&item) {
            constraints.push(item);
        } else {
            columns.push(item);
        }
    }
    constraints.sort();
    let parts: Vec<String> = columns.into_iter().chain(constraints).collect();
    format!(
        "{} (\n\t{}\n){}",
        head.trim_end(),
        parts.join(",\n\t"),
        tail.trim_end()
    )
}

fn canonical_document(conn: &Connection) -> Result<Json, Box<dyn Error>> {
    let schema = {
        let mut stmt = conn
            .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut schema = Vec::new();
        for row in rows {
            let (kind, name, table, sql) = row?;
            let sql = normalised_sql(&kind, sql);
            schema.push(object([
                ("type", Json::Str(kind)),
                ("name", Json::Str(name)),
                ("table", Json::Str(table)),
                ("sql", sql.map_or(Json::Null, Json::Str)),
            ]));
        }
        Json::Arr(schema)
    };

    let table_names: Vec<String> = {
        let mut stmt =
            conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
        let names = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut kept = Vec::new();
        for name in names {
            let name = name?;
            if !is_excluded(&name) {
                kept.push(name);
            }
        }
        kept
    };
    let mut tables = BTreeMap::new();
    for name in &table_names {
        tables.insert(name.clone(), table_document(conn, name)?);
    }

    Ok(object([("schema", schema), ("tables", Json::Obj(tables))]))
}

fn table_document(conn: &Connection, table: &str) -> Result<Json, Box<dyn Error>> {
    let quoted = quote(table);
    let info: Vec<(String, i64)> = {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({quoted})"))?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    let mut primary_key: Vec<(i64, &str)> = info
        .iter()
        .filter(|(_, pk)| *pk != 0)
        .map(|(name, pk)| (*pk, name.as_str()))
        .collect();
    primary_key.sort();
    let order = if primary_key.is_empty() {
        "rowid".to_owned()
    } else {
        join_quoted(primary_key.iter().map(|(_, name)| *name))
    };
    let names: Vec<&str> = info.iter().map(|(name, _)| name.as_str()).collect();
    let select = join_quoted(names.iter().copied());

    let mut stmt = conn.prepare(&format!("SELECT {select} FROM {quoted} ORDER BY {order}"))?;
    let mut cursor = stmt.query([])?;
    let mut rows = Vec::new();
    while let Some(row) = cursor.next()? {
        let mut values = BTreeMap::new();
        for (index, name) in names.iter().enumerate() {
            values.insert((*name).to_owned(), canonical_value(row.get_ref(index)?)?);
        }
        rows.push(Json::Obj(values));
    }
    Ok(object([
        (
            "columns",
            Json::Arr(names.iter().map(|n| Json::Str((*n).to_owned())).collect()),
        ),
        ("rows", Json::Arr(rows)),
    ]))
}

fn canonical_value(value: ValueRef<'_>) -> Result<Json, Box<dyn Error>> {
    Ok(match value {
        ValueRef::Null => Json::Null,
        ValueRef::Integer(number) => Json::Int(number.to_string()),
        ValueRef::Real(number) => Json::Float(number),
        ValueRef::Text(bytes) => {
            let text = std::str::from_utf8(bytes)?;
            if text.trim_start().starts_with(['{', '[']) {
                parse_json(text).unwrap_or_else(|| Json::Str(text.to_owned()))
            } else {
                Json::Str(text.to_owned())
            }
        }
        ValueRef::Blob(bytes) => {
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            object([("blob", Json::Str(hex))])
        }
    })
}

fn normalised_sql(kind: &str, sql: Option<String>) -> Option<String> {
    let sql = sql?;
    Some(if kind == "table" {
        canonical_create_table(&sql)
    } else {
        sql.trim_end().trim_end_matches(';').to_owned()
    })
}

fn is_excluded(name: &str) -> bool {
    EXCLUDED_TABLES.contains(&name) || EXCLUDED_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn join_quoted<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names.map(quote).collect::<Vec<_>>().join(", ")
}

fn object<const N: usize>(pairs: [(&str, Json); N]) -> Json {
    Json::Obj(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

/// `_split_top_level`: split a table body at commas outside parentheses and quotes, then collapse
/// the whitespace of each item.
fn split_top_level(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth: i64 = 0;
    let mut quote: Option<char> = None;
    let mut current = String::new();
    for ch in body.chars() {
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
        } else if ch == ',' && depth == 0 {
            items.push(std::mem::take(&mut current));
            continue;
        }
        current.push(ch);
    }
    items.push(current);
    items
        .into_iter()
        .filter(|item| !item.trim().is_empty())
        .map(|item| item.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// `_CONSTRAINT_START`: `(CONSTRAINT|PRIMARY|UNIQUE|FOREIGN|CHECK)\b`, case-insensitive, at the
/// start of an item.
fn is_constraint_start(item: &str) -> bool {
    ["CONSTRAINT", "PRIMARY", "UNIQUE", "FOREIGN", "CHECK"]
        .iter()
        .any(|keyword| {
            item.get(..keyword.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(keyword))
                && item[keyword.len()..]
                    .chars()
                    .next()
                    .is_none_or(|next| !(next.is_alphanumeric() || next == '_'))
        })
}

/// Reads one JSON document the way Python's `json.loads` does: JSON whitespace only, `NaN`,
/// `Infinity` and `-Infinity` accepted, control characters in strings refused.
fn parse_json(text: &str) -> Option<Json> {
    let mut reader = JsonReader { rest: text };
    reader.ws();
    let value = reader.value()?;
    reader.ws();
    reader.rest.is_empty().then_some(value)
}

struct JsonReader<'a> {
    rest: &'a str,
}

impl JsonReader<'_> {
    fn ws(&mut self) {
        self.rest = self.rest.trim_start_matches([' ', '\t', '\n', '\r']);
    }

    fn eat(&mut self, token: &str) -> bool {
        match self.rest.strip_prefix(token) {
            Some(rest) => {
                self.rest = rest;
                true
            }
            None => false,
        }
    }

    fn literal(&mut self, token: &str, value: Json) -> Option<Json> {
        self.eat(token).then_some(value)
    }

    fn value(&mut self) -> Option<Json> {
        let first = self.rest.chars().next()?;
        match first {
            '"' => {
                self.rest = &self.rest[1..];
                self.string().map(Json::Str)
            }
            '{' => {
                self.rest = &self.rest[1..];
                self.object()
            }
            '[' => {
                self.rest = &self.rest[1..];
                self.array()
            }
            'n' => self.literal("null", Json::Null),
            't' => self.literal("true", Json::Bool(true)),
            'f' => self.literal("false", Json::Bool(false)),
            'N' => self.literal("NaN", Json::Float(f64::NAN)),
            'I' => self.literal("Infinity", Json::Float(f64::INFINITY)),
            '-' if self.rest.starts_with("-Infinity") => {
                self.literal("-Infinity", Json::Float(f64::NEG_INFINITY))
            }
            _ => self.number(),
        }
    }

    fn object(&mut self) -> Option<Json> {
        let mut map = BTreeMap::new();
        self.ws();
        if self.eat("}") {
            return Some(Json::Obj(map));
        }
        loop {
            self.ws();
            if !self.eat("\"") {
                return None;
            }
            let key = self.string()?;
            self.ws();
            if !self.eat(":") {
                return None;
            }
            self.ws();
            map.insert(key, self.value()?);
            self.ws();
            if self.eat(",") {
                continue;
            }
            return self.eat("}").then_some(Json::Obj(map));
        }
    }

    fn array(&mut self) -> Option<Json> {
        let mut items = Vec::new();
        self.ws();
        if self.eat("]") {
            return Some(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value()?);
            self.ws();
            if self.eat(",") {
                continue;
            }
            return self.eat("]").then_some(Json::Arr(items));
        }
    }

    /// The body of a string whose opening quote has been consumed.
    fn string(&mut self) -> Option<String> {
        let mut out = String::new();
        loop {
            let ch = self.rest.chars().next()?;
            self.rest = &self.rest[ch.len_utf8()..];
            match ch {
                '"' => return Some(out),
                '\\' => {
                    let escape = self.rest.chars().next()?;
                    self.rest = &self.rest[escape.len_utf8()..];
                    match escape {
                        '"' | '\\' | '/' => out.push(escape),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => out.push(self.unicode_escape()?),
                        _ => return None,
                    }
                }
                c if (c as u32) < 0x20 => return None,
                c => out.push(c),
            }
        }
    }

    /// The code point of a `\u` escape, joining a surrogate pair. A lone surrogate cannot be
    /// written as UTF-8, so Python's dump of it fails; it is refused here.
    fn unicode_escape(&mut self) -> Option<char> {
        let unit = self.hex4()?;
        let code = if (0xD800..=0xDBFF).contains(&unit) {
            if !self.eat("\\u") {
                return None;
            }
            let low = self.hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return None;
            }
            0x1_0000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
        } else {
            unit
        };
        char::from_u32(code)
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.rest.get(..4)?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.rest = &self.rest[4..];
        u32::from_str_radix(digits, 16).ok()
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`. An integer keeps its digits, with `-0`
    /// written as Python writes the int 0.
    fn number(&mut self) -> Option<Json> {
        let text = self.rest;
        let bytes = text.as_bytes();
        let digits_at = |mut i: usize| {
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            i
        };
        let mut i = usize::from(bytes.first() == Some(&b'-'));
        match bytes.get(i) {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => i = digits_at(i),
            _ => return None,
        }
        let mut is_float = false;
        if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            i = digits_at(i + 1);
            is_float = true;
        }
        if matches!(bytes.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(bytes.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if bytes.get(j).is_some_and(u8::is_ascii_digit) {
                i = digits_at(j);
                is_float = true;
            }
        }
        let literal = &text[..i];
        self.rest = &text[i..];
        if is_float {
            literal.parse::<f64>().ok().map(Json::Float)
        } else if literal == "-0" {
            Some(Json::Int("0".to_owned()))
        } else {
            Some(Json::Int(literal.to_owned()))
        }
    }
}

/// `json.dumps(value, sort_keys=True, indent=1, ensure_ascii=False)`.
fn write_value(value: &Json, depth: usize, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(digits) => out.push_str(digits),
        Json::Float(number) => out.push_str(&float_text(*number)),
        Json::Str(text) => write_string(text, out),
        Json::Arr(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(depth + 1, out);
                write_value(item, depth + 1, out);
            }
            newline(depth, out);
            out.push(']');
        }
        Json::Obj(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(depth + 1, out);
                write_string(key, out);
                out.push_str(": ");
                write_value(item, depth + 1, out);
            }
            newline(depth, out);
            out.push('}');
        }
    }
}

fn newline(depth: usize, out: &mut String) {
    out.push('\n');
    for _ in 0..depth {
        out.push(' ');
    }
}

/// Python's `float.__repr__`, with `json`'s spellings for the non-finite values.
fn float_text(number: f64) -> String {
    if number.is_nan() {
        "NaN".to_owned()
    } else if number == f64::INFINITY {
        "Infinity".to_owned()
    } else if number == f64::NEG_INFINITY {
        "-Infinity".to_owned()
    } else {
        py_float_repr(number)
    }
}

/// `json`'s string encoder with `ensure_ascii=False`.
fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
