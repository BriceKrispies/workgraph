"""Generates crates/wg-storecodec/src/rows_more.rs: codecs for the tables beyond the first 18.

Reads the entity structs (crates/wg-model/src/entity), Table names (write.rs) and the column
lists and keys of contracts/schema-0011.sql. Run from anywhere: python tools/gen_storecodec_rows.py
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


# --- model: Table variants in order, names ---
write_rs = read("crates/wg-model/src/write.rs")
variants = re.findall(r'^\s+Table::(\w+) => "(\w+)",', write_rs, re.M)
sql_name = dict(variants)
order = [v for v, _ in variants]
FIRST = order[:18]
MORE = order[18:]

# --- structs ---
structs = {}
for path in sorted((ROOT / "crates/wg-model/src/entity").glob("*.rs")):
    text = path.read_text(encoding="utf-8")
    for m in re.finditer(r"pub struct (\w+) \{\n(.*?)\n\}", text, re.S):
        fields = re.findall(r"^    pub (\w+): (.+),$", m.group(2), re.M)
        structs[m.group(1)] = fields

# --- schema columns / pks ---
schema = read("contracts/schema-0011.sql")


def schema_table(name):
    for blk in schema.split("\n\n"):
        if blk.startswith((f"CREATE TABLE {name} (", f'CREATE TABLE "{name}" (')):
            cols = []
            for line in blk.splitlines()[1:]:
                line = line.strip()
                if line.startswith(("CONSTRAINT", ")")):
                    if line.startswith(")"):
                        break
                    continue
                cols.append(line.split()[0].strip('"'))
            pk = re.search(r"PRIMARY KEY \(([^)]*)\)", blk).group(1).split(", ")
            return cols, pk
    raise KeyError(name)


ENUMS = set()


def enc(f, ty):
    s = f"self.{f}"
    table = {
        "Id": f"id_sql({s})",
        "Option<Id>": f"opt_id_sql({s})",
        "String": f"text_sql(&{s})",
        "Option<String>": f"opt_text_sql(&{s})",
        "Timestamp": f"ts_sql({s})",
        "Option<Timestamp>": f"opt_ts_sql({s})",
        "i64": f"Raw::Int({s})",
        "Option<i64>": f"opt_int_sql({s})",
        "bool": f"bool_sql({s})",
        "JsonObject": f"object_sql(&{s})?",
        "Option<JsonObject>": f"opt_object_sql(&{s})?",
        "VectorSync<String>": f"strings_sql(&{s})?",
        "VectorSync<JsonObject>": f"objects_sql(&{s})?",
        "VectorSync<Value>": f"values_sql(&{s})?",
        "VectorSync<u8>": f"blob_sql(&{s})",
        "Real": f"real_sql({s})",
        "Option<Real>": f"opt_real_sql({s})",
    }
    if ty in table:
        return table[ty]
    ENUMS.add(ty)
    return f"text_sql({s}.as_str())"


def dec(f, ty):
    c = {
        "Id": "id",
        "Option<Id>": "opt_id",
        "String": "text",
        "Option<String>": "opt_text",
        "Timestamp": "ts",
        "Option<Timestamp>": "opt_ts",
        "i64": "int",
        "Option<i64>": "opt_int",
        "bool": "flag",
        "JsonObject": "object",
        "Option<JsonObject>": "opt_object",
        "VectorSync<String>": "strings",
        "VectorSync<JsonObject>": "objects",
        "VectorSync<Value>": "values",
        "VectorSync<u8>": "blob",
        "Real": "real",
        "Option<Real>": "opt_real",
    }.get(ty, "parsed")
    return f'c.{c}("{f}")?'


PK_EXTRA = {}
out = []
out.append("""//! Codecs for the execution and knowledge tables (generated from the model's entity structs and
//! `contracts/schema-0011.sql` by `tools/gen_storecodec_rows.py`; edit the generator, not this file).
//!
//! Each table has its `columns` list (schema order, checked against the schema by the crate's
//! tests), its primary key, and a [`TableRow`] implementation.

use wg_model::entity::*;
use wg_model::{Row, RowKey, Table};
use wg_types::WgError;

use crate::codec::{
    Cols, Raw, blob_sql, bool_sql, id_sql, object_sql, objects_sql, opt_id_sql, opt_int_sql,
    opt_object_sql, opt_real_sql, opt_text_sql, opt_ts_sql, real_sql, strings_sql, text_sql, ts_sql,
    values_sql,
};
use crate::rows::TableRow;
""")

# columns_more
out.append("/// Columns of the tables beyond the first 18, in schema order; empty for those 18.")
out.append("pub(crate) const fn columns_more(table: Table) -> &'static [&'static str] {\n    match table {")
infos = {}
for v in MORE:
    cols, pk = schema_table(sql_name[v])
    fields = structs[v]
    assert [f for f, _ in fields] == cols, (v, [f for f, _ in fields], cols)
    infos[v] = (cols, pk, fields)
    out.append(f"        Table::{v} => &[" + ", ".join(f'"{c}"' for c in cols) + "],")
out.append("        " + " | ".join(f"Table::{v}" for v in FIRST) + " => &[],")
out.append("    }\n}\n")

# primary_key_more: None when default
out.append("/// Primary-key columns of the tables beyond the first 18 (`id` unless listed).")
out.append(
    "pub(crate) const fn primary_key_more(table: Table) -> &'static [&'static str] {\n    match table {"
)
for v in MORE:
    cols, pk, _ = infos[v]
    if pk != ["id"]:
        out.append(f"        Table::{v} => &[" + ", ".join(f'"{c}"' for c in pk) + "],")
out.append('        _ => &["id"],\n    }\n}\n')

# key_values_more
rowkeys = {}
for m in re.finditer(
    r"^    (\w+)\((\w+)\),$", re.search(r"pub enum RowKey \{(.*?)\n\}", write_rs, re.S).group(1), re.M
):
    rowkeys[m.group(1)] = m.group(2)
out.append("/// Primary-key values of keys of the tables beyond the first 18; empty for those 18.")
out.append("pub(crate) fn key_values_more(key: &RowKey) -> Vec<Raw> {\n    match key {")
idv = [v for v in MORE if rowkeys.get(v) == "Id"]
strv = [v for v in MORE if rowkeys.get(v) == "String"]
out.append("        " + " | ".join(f"RowKey::{v}(id)" for v in idv) + " => vec![id_sql(*id)],")
out.append("        " + " | ".join(f"RowKey::{v}(text)" for v in strv) + " => vec![text_sql(text)],")
out.append(
    "        RowKey::StackRepository { stack_id, repository_id } => vec![id_sql(*stack_id), id_sql(*repository_id)],"
)
out.append(
    "        RowKey::JobResourceClaim { job_id, resource_key } => vec![id_sql(*job_id), text_sql(resource_key)],"
)
out.append(
    "        "
    + " | ".join(f"RowKey::{v}" + ("(_)" if rowkeys.get(v) else " { .. }") for v in FIRST)
    + " => Vec::new(),"
)
out.append("    }\n}\n")
assert set(idv) | set(strv) | {"StackRepository", "JobResourceClaim"} == set(MORE), (
    set(MORE) - set(idv) - set(strv)
)

# row_values_more
out.append("/// Bound parameters of rows of the tables beyond the first 18; empty for those 18.")
out.append("pub(crate) fn row_values_more(row: &Row) -> Result<Vec<Raw>, WgError> {\n    match row {")
for v in MORE:
    out.append(f"        Row::{v}(row) => row.values(),")
out.append("        " + " | ".join(f"Row::{v}(_)" for v in FIRST) + " => Ok(Vec::new()),")
out.append("    }\n}\n")

# impls
for v in MORE:
    cols, pk, fields = infos[v]
    out.append(f"impl TableRow for {v} {{\n    const TABLE: Table = Table::{v};\n")
    out.append("    fn values(&self) -> Result<Vec<Raw>, WgError> {\n        Ok(vec![")
    for f, t in fields:
        out.append(f"            {enc(f, t)},")
    out.append("        ])\n    }\n")
    out.append("    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {\n        Ok(" + v + " {")
    for f, t in fields:
        out.append(f"            {f}: {dec(f, t)},")
    out.append("        })\n    }\n}\n")

(ROOT / "crates/wg-storecodec/src/rows_more.rs").write_text(
    "\n".join(out) + "\n", encoding="utf-8", newline="\n"
)
print("enums", ENUMS, "more", len(MORE), "first", len(FIRST))
