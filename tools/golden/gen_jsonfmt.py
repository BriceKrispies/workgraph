"""Golden vectors GV-12 (`jsonfmt`): JSON and timestamp formatting in the Python dialects.

Writes ``golden/jsonfmt/cases.json``. Every output is produced by the real Python code:

* O01 (CLI ``--json``): ``workgraph.cli.common.emit`` composition, i.e.
  ``json.dumps(to_jsonable(value), indent=2, default=str) + "\\n"``.
* O06 (HTTP): ``starlette.responses.JSONResponse(to_jsonable(value)).body``.
* O10 (DB JSON columns): ``workgraph.persistence.types.JSONText`` bind processing.
* isoformat: ``datetime.isoformat()``.
* when: ``workgraph.cli.common.when``.

Inputs are stored as a tagged tree (see ``build`` below) so the Rust side can rebuild the same
Python-level value without evaluating Python. Floats are stored as their ``repr`` text, datetimes
as ISO strings parsed with ``datetime.fromisoformat``. Errors are recorded by exception class name.

Run: ``uv run python tools/golden/gen_jsonfmt.py``. Output is deterministic (sorted keys, ``\\n``
line endings, UTF-8, no timestamps of the run).
"""

from __future__ import annotations

import json
from datetime import date, datetime
from decimal import Decimal
from pathlib import Path
from typing import Any
from uuid import UUID

from pydantic import BaseModel
from starlette.responses import JSONResponse

from workgraph.cli.common import to_jsonable, when
from workgraph.persistence.types import JSONText

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "jsonfmt"

DIALECTS = {
    "O01": "CLI --json: json.dumps(to_jsonable(v), indent=2, default=str) + newline",
    "O06": "HTTP: starlette JSONResponse(to_jsonable(v)).body (compact, ensure_ascii=False, allow_nan=False)",
    "O10": "DB JSON column: json.dumps(v, sort_keys=True, separators=(',', ':'), default=str)",
    "isoformat": "datetime.isoformat()",
    "when": "cli.common.when(v): strftime('%Y-%m-%d %H:%M') or ''",
}


class Inner(BaseModel):
    kind: str
    at: datetime


class Sample(BaseModel):
    # Declaration order is deliberately not alphabetical: JSON output must follow it.
    zeta: str
    created_at: datetime
    alpha: int
    ratio: float
    note: str | None
    updated_at: datetime
    tags: list[str]
    inner: Inner


MODELS: dict[str, type[BaseModel]] = {"Sample": Sample, "Inner": Inner}


# --- tagged input tree -------------------------------------------------------------------------


def s(v: str) -> dict[str, Any]:
    return {"t": "str", "v": v}


def f(repr_text: str) -> dict[str, Any]:
    """A float given by its Python repr ("nan", "inf", "-inf", "1e-05", "0.1", ...)."""
    return {"t": "float", "v": repr_text}


def n(v: int) -> dict[str, Any]:
    return {"t": "int", "v": v}


def dt(iso: str) -> dict[str, Any]:
    return {"t": "datetime", "v": iso}


def d(iso: str) -> dict[str, Any]:
    return {"t": "date", "v": iso}


def dec(text: str) -> dict[str, Any]:
    return {"t": "decimal", "v": text}


def uid(text: str) -> dict[str, Any]:
    return {"t": "uuid", "v": text}


def raw_bytes(hex_text: str) -> dict[str, Any]:
    return {"t": "bytes", "v": hex_text}


def lst(*items: dict[str, Any]) -> dict[str, Any]:
    return {"t": "list", "v": list(items)}


def tup(*items: dict[str, Any]) -> dict[str, Any]:
    return {"t": "tuple", "v": list(items)}


def obj(*pairs: tuple[str, dict[str, Any]]) -> dict[str, Any]:
    """A dict; pairs keep insertion order, so sort_keys behaviour is exercised."""
    return {"t": "dict", "v": [[k, v] for k, v in pairs]}


def model(name: str, *pairs: tuple[str, dict[str, Any]]) -> dict[str, Any]:
    return {"t": "model", "name": name, "v": [[k, v] for k, v in pairs]}


NONE = {"t": "null"}


def build(node: dict[str, Any]) -> Any:
    t = node["t"]
    if t == "null":
        return None
    if t == "str":
        return node["v"]
    if t == "bool":
        return bool(node["v"])
    if t == "int":
        return int(node["v"])
    if t == "float":
        return float(node["v"])
    if t == "datetime":
        return datetime.fromisoformat(node["v"])
    if t == "date":
        return date.fromisoformat(node["v"])
    if t == "decimal":
        return Decimal(node["v"])
    if t == "uuid":
        return UUID(node["v"])
    if t == "bytes":
        return bytes.fromhex(node["v"])
    if t == "list":
        return [build(x) for x in node["v"]]
    if t == "tuple":
        return tuple(build(x) for x in node["v"])
    if t == "dict":
        return {k: build(x) for k, x in node["v"]}
    if t == "model":
        return MODELS[node["name"]](**{k: build(x) for k, x in node["v"]})
    raise ValueError(f"unknown node type {t!r}")


# --- dialect runners ---------------------------------------------------------------------------


def run(dialect: str, value: Any) -> str:
    if dialect == "O01":
        return json.dumps(to_jsonable(value), indent=2, default=str) + "\n"
    if dialect == "O06":
        return JSONResponse(to_jsonable(value)).body.decode("utf-8")
    if dialect == "O10":
        return JSONText().process_bind_param(value, None)
    if dialect == "isoformat":
        return value.isoformat()
    if dialect == "when":
        return when(value)
    raise ValueError(dialect)


def outputs_for(node: dict[str, Any], dialects: list[str]) -> dict[str, dict[str, str]]:
    out: dict[str, dict[str, str]] = {}
    for dialect in dialects:
        try:
            value = build(node)
            out[dialect] = {"output": run(dialect, value)}
        except Exception as exc:  # the error class is the golden result
            out[dialect] = {"error": type(exc).__name__}
    return out


# --- case table --------------------------------------------------------------------------------

JSON_DIALECTS = ["O01", "O06", "O10"]
DT_DIALECTS = ["O01", "O06", "O10", "isoformat"]
WHEN_DIALECTS = ["when"]

STRINGS: list[tuple[str, str]] = [
    ("quote", 'say "hi"'),
    ("backslash", "a\\b\\\\c"),
    ("controls-short", "\b\f\n\r\t"),
    ("controls-nul-unit", "\x00\x01\x07\x0b\x1e\x1f"),
    ("del", "\x7f"),
    ("slash", "a/b</script>"),
    ("latin", "café naïve"),
    ("cjk", "日本語"),
    ("emoji-surrogate-pair", "a😀b"),
    ("line-separators", chr(0x2028) + chr(0x2029)),
    ("bom", chr(0xFEFF) + "x"),
    ("max-bmp", chr(0xFFFF)),
    ("combining", "é"),
    ("lone-surrogate", "\ud800"),
    ("empty", ""),
    ("edge-spaces", "  x  "),
    ("json-inside", '{"a": [1, 2.5, null]}'),
]

FLOATS: list[str] = [
    "0.0",
    "-0.0",
    "1.0",
    "2.5",
    "0.1",
    "3.141592653589793",
    "0.0001",
    "1e-05",
    "5e-324",
    "1e+16",
    "1000000000000000.0",
    "9999999999999998.0",
    "123456789.123",
    "1.5e+300",
    "1.7976931348623157e+308",
    "nan",
    "inf",
    "-inf",
]

DATETIMES: list[str] = [
    "2026-10-25T13:56:00+00:00",
    "2026-10-25T13:56:00Z",
    "2026-10-25T13:56:00.123456+00:00",
    "2026-10-25T13:56:00.100000+00:00",
    "2026-10-25T13:56:00.000001+00:00",
    "2026-10-25T15:00:00+02:00",
    "2026-10-25T08:26:00-05:30",
    "2026-10-25T13:56:00",
    "2026-02-28T23:59:59.999999+00:00",
    "2024-02-29T00:00:00+00:00",
    "0001-01-01T00:00:00+00:00",
    "9999-12-31T23:59:59.999999+00:00",
]

WHEN_VALUES: list[str] = [
    "2026-10-25T13:56:00+00:00",
    "2026-10-25T13:56:59.999999+00:00",
    "2026-10-25T15:00:00+02:00",
    "2026-01-01T00:00:00",
    "2024-02-29T09:05:00+00:00",
    "2026-12-31T23:59:30-05:30",
]


def cases() -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []

    def add(cid: str, group: str, node: dict[str, Any], dialects: list[str]) -> None:
        out.append({"id": cid, "group": group, "input": node, "outputs": outputs_for(node, dialects)})

    for name, text in STRINGS:
        add(f"string-{name}", "string", s(text), JSON_DIALECTS)

    add(
        "string-keys-sorted",
        "dict",
        obj(("b", s("1")), ("a", s("\n")), ("É", s("x")), ("z", n(0)), ("ä", n(2))),
        JSON_DIALECTS,
    )
    add("dict-empty", "dict", obj(), JSON_DIALECTS)
    add("list-empty", "list", lst(), JSON_DIALECTS)
    add("list-mixed", "list", lst(n(1), s("two"), NONE, f("3.5"), lst(), obj()), JSON_DIALECTS)
    add("null", "scalar", NONE, JSON_DIALECTS)
    add("bool-true", "scalar", {"t": "bool", "v": True}, JSON_DIALECTS)
    add("bool-false", "scalar", {"t": "bool", "v": False}, JSON_DIALECTS)

    for i, text in enumerate(FLOATS):
        add(f"float-{i:02d}-{text}", "float", f(text), JSON_DIALECTS)

    for label, value in [
        ("zero", "0"),
        ("negative", "-1"),
        ("2pow64-plus5", str(2**64 + 5)),
        ("10pow30", str(10**30)),
    ]:
        add(f"int-{label}", "int", n(int(value)), JSON_DIALECTS)

    # Nested default=str values (datetime, date, Decimal, UUID, bytes, tuple) in plain containers.
    add(
        "nested-default-str",
        "nested",
        obj(
            ("when", dt("2026-10-25T13:56:00+00:00")),
            ("naive", dt("2026-10-25T13:56:00")),
            ("day", d("2026-10-25")),
            ("amount", dec("1.10")),
            ("id", uid("0f8fad5b-d9cb-469f-a165-8c0d8b6a6f31")),
            ("raw", raw_bytes("00616200")),
            ("items", tup(n(1), n(2))),
            ("deep", obj(("list", lst(dt("2026-10-25T15:00:00+02:00"))))),
        ),
        JSON_DIALECTS,
    )
    add(
        "nested-default-str-decimal-list",
        "nested",
        lst(dec("0.30"), dec("-2E+3"), uid("00000000-0000-4000-8000-000000000000")),
        JSON_DIALECTS,
    )

    for i, iso in enumerate(DATETIMES):
        add(f"datetime-{i:02d}", "datetime", dt(iso), DT_DIALECTS)

    for i, iso in enumerate(WHEN_VALUES):
        add(f"when-{i:02d}", "when", dt(iso), WHEN_DIALECTS)
    add("when-none", "when", NONE, WHEN_DIALECTS)

    add(
        "model-sample-aware-utc",
        "model",
        model(
            "Sample",
            ("zeta", s("last-declared-first-field")),
            ("created_at", dt("2026-10-25T13:56:00+00:00")),
            ("alpha", n(1)),
            ("ratio", f("0.5")),
            ("note", NONE),
            ("updated_at", dt("2026-10-25T13:56:00.123456+00:00")),
            ("tags", lst(s("a"), s("é"))),
            ("inner", model("Inner", ("kind", s("k")), ("at", dt("2026-10-25T15:00:00+02:00")))),
        ),
        JSON_DIALECTS,
    )
    add(
        "model-sample-naive-and-nan",
        "model",
        model(
            "Sample",
            ("zeta", s("")),
            ("created_at", dt("2026-10-25T13:56:00")),
            ("alpha", n(-7)),
            ("ratio", f("nan")),
            ("note", s('n"ote')),
            ("updated_at", dt("2026-10-25T13:56:00.100000+00:00")),
            ("tags", lst()),
            ("inner", model("Inner", ("kind", s("")), ("at", dt("0001-01-01T00:00:00+00:00")))),
        ),
        JSON_DIALECTS,
    )
    add(
        "model-inner-only",
        "model",
        model("Inner", ("kind", s("x\ty")), ("at", dt("2026-02-28T23:59:59.999999+00:00"))),
        JSON_DIALECTS,
    )

    return out


def main() -> None:
    doc = {
        "vector": "jsonfmt",
        "task": "GV-12",
        "dialects": DIALECTS,
        "cases": cases(),
    }
    text = json.dumps(doc, sort_keys=True, indent=2, ensure_ascii=True) + "\n"
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    (OUT_DIR / "cases.json").write_bytes(text.encode("utf-8"))
    print(f"wrote {OUT_DIR / 'cases.json'}: {len(doc['cases'])} cases")


if __name__ == "__main__":
    main()
