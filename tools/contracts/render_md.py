"""Regenerate section 13 of ``docs/rust-rewrite-contracts.md`` from the snapshots (task CF-02).

Inputs:

* ``contracts/cli.json``, ``contracts/mcp.json``, ``contracts/openapi.json``,
  ``contracts/config-schema.json`` and ``contracts/schema-0011.sql`` (the CF-01 snapshots);
* the first line of each CLI command's docstring under ``src/workgraph/cli`` (read only). Help text is
  not part of the snapshot by design (section 11, decision 1), so the Summary column of 13.1 comes
  from the source the help is written in.

Rules:

* IDs (``C001``, ``M01``, ``H001``) are stable. The committed section is the registry: a command,
  tool or operation keeps the ID it already has, and a new one takes the next free number. Renumbering
  would invalidate the coverage ranges that section 9 of ``docs/rust-rewrite.md`` names.
* Row order follows the committed order for existing rows; new rows are appended.
* The MCP ``Parameters`` column keeps the committed parameter order for parameters that still
  exist (the snapshot stores properties sorted by name). New parameters are appended.
* Prose, the routes outside OpenAPI (X1-X7) and the typed sub-table list are kept as written. The
  counts quoted in the prose are checked against the snapshots and the script fails if they differ.

Usage::

    uv run python tools/contracts/render_md.py          # rewrite section 13
    uv run python tools/contracts/render_md.py --check  # exit 1 if section 13 would change

Running it twice gives identical bytes.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sqlite3
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
CONTRACTS_DIR = ROOT / "contracts"
DOC_PATH = ROOT / "docs" / "rust-rewrite-contracts.md"
CLI_SRC_DIR = ROOT / "src" / "workgraph" / "cli"
SECTION_HEADING = "## 13. Generated inventory"

CLI_HEADER = "| # | Command | Arguments and options | Summary |"
MCP_HEADER = "| # | Tool | Parameters | Description |"
HTTP_HEADER = "| # | Method | Path | OK status | Query | Body | Response | Summary |"
DB_HEADER = "| Table | Columns | Named indexes |"
CONFIG_HEADER = "| Key | Type | Default | Allowed |"


class RenderError(Exception):
    """A snapshot and the committed prose disagree, or a row cannot be rendered."""


# ---------------------------------------------------------------- CLI summaries (source docstrings)


def cli_summaries(cli_src: Path) -> dict[str, str]:
    """Map ``workgraph <group> <command>`` to the first docstring line of its function."""
    parent_of: dict[str, tuple[str, str]] = {}
    add_typer = re.compile(r'(\w+)\.add_typer\(\s*([\w.]+)\s*,\s*name="([^"]+)"')
    files = sorted(cli_src.rglob("*.py"))
    for path in files:
        for match in add_typer.finditer(path.read_text(encoding="utf-8")):
            parent_of[match.group(2).split(".")[-1]] = (match.group(1), match.group(3))

    def group(var: str) -> list[str]:
        if var not in parent_of:
            return []
        parent, name = parent_of[var]
        return [*group(parent), name]

    functions: dict[str, str] = {}
    registrations: list[tuple[list[str], str, str]] = []  # (group, command name, function name)
    for path in files:
        tree = ast.parse(path.read_text(encoding="utf-8"))
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef):
                functions.setdefault(node.name, (ast.get_docstring(node) or "").split("\n")[0].strip())
                for dec in node.decorator_list:
                    if (
                        isinstance(dec, ast.Call)
                        and isinstance(dec.func, ast.Attribute)
                        and dec.func.attr == "command"
                        and isinstance(dec.func.value, ast.Name)
                    ):
                        name = (
                            dec.args[0].value
                            if dec.args and isinstance(dec.args[0], ast.Constant)
                            else node.name.replace("_", "-")
                        )
                        registrations.append((group(dec.func.value.id), name, node.name))
            # Root commands registered as ``app.command("name")(function)``.
            if (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Call)
                and isinstance(node.func.func, ast.Attribute)
                and node.func.func.attr == "command"
                and isinstance(node.func.func.value, ast.Name)
                and node.func.args
                and isinstance(node.func.args[0], ast.Constant)
                and node.args
                and isinstance(node.args[0], ast.Name | ast.Attribute)
            ):
                target = node.args[0]
                fn_name = target.id if isinstance(target, ast.Name) else target.attr
                registrations.append((group(node.func.func.value.id), node.func.args[0].value, fn_name))
    out: dict[str, str] = {}
    for grp, name, fn in registrations:
        key = " ".join(["workgraph", *grp, name])
        if fn not in functions:
            raise RenderError(f"no function {fn} for command {key}")
        out[key] = functions[fn]
    return out


# ---------------------------------------------------------------- committed registry


def _committed_section(doc_text: str) -> str:
    index = doc_text.find(SECTION_HEADING)
    if index < 0:
        raise RenderError(f"{DOC_PATH} has no '{SECTION_HEADING}' heading")
    return doc_text[index:]


def _registry(section: str) -> dict[str, Any]:
    """Read the IDs and orders the committed section already assigns."""
    cli_ids: dict[str, str] = {}
    mcp_ids: dict[str, str] = {}
    mcp_params: dict[str, list[str]] = {}
    http_ids: dict[tuple[str, str], str] = {}
    config_order: dict[str, int] = {}
    config_defaults: dict[str, str] = {}
    for line in section.splitlines():
        if m := re.match(r"^\| (C\d{3}) \| `([^`]+)` \|", line):
            cli_ids[m.group(2)] = m.group(1)
        elif m := re.match(r"^\| (M\d{2}) \| `([^`]+)` \| (.*?) \| (.*) \|$", line):
            mcp_ids[m.group(2)] = m.group(1)
            if m.group(3).strip() != "-":
                mcp_params[m.group(2)] = re.findall(r"(?:^|, )!?(\w+): ", m.group(3))
        elif m := re.match(r"^\| (H\d{3}) \| (\w+) \| `([^`]+)` \|", line):
            http_ids[(m.group(2), m.group(3))] = m.group(1)
        elif m := re.match(r"^\| `([^`]+)` \| (.*?) \| (.*?) \| (.*?) \|$", line):
            config_order.setdefault(m.group(1), len(config_order))
            config_defaults[m.group(1)] = m.group(3)
    return {
        "cli": cli_ids,
        "mcp": mcp_ids,
        "mcp_params": mcp_params,
        "http": http_ids,
        "config": config_order,
        "config_defaults": config_defaults,
    }


def _assign_ids(keys: list[Any], known: dict[Any, str], prefix: str, width: int) -> dict[Any, str]:
    """Keep known IDs; give new keys the next free numbers in snapshot order."""
    used = {int(v[len(prefix) :]) for v in known.values()}
    next_no = max(used, default=0) + 1
    ids: dict[Any, str] = {}
    for key in keys:
        if key in known:
            ids[key] = known[key]
        else:
            ids[key] = f"{prefix}{next_no:0{width}d}"
            next_no += 1
    return ids


# ---------------------------------------------------------------- CLI table


def _cli_param(param: dict[str, Any]) -> str:
    if param["kind"] == "argument":
        name = param["name"].upper()
        if param.get("nargs") == -1:
            return f"{name}..."
        return name if param["required"] else f"[{name}]"
    if param["is_flag"]:
        text = "`" + "/".join(param["opts"] + param["secondary_opts"]) + "`"
        return text + (" =True" if param["default"] is True else "")
    text = f"`{'/'.join(param['opts'])}`"
    if param["choices"]:
        text += " {" + ",".join(str(c) for c in param["choices"]) + "}"
    else:
        text += f" {param['type']}"
    if param["multiple"]:
        text += " *"
    if param["required"]:
        text += " required"
    if param["default"] is not None and not param["choices"]:
        text += f" ={param['default']}"
    if param["envvar"]:
        text += f" env {param['envvar']}"
    return text


def _cli_rows(commands: list[dict[str, Any]], summaries: dict[str, str], ids: dict[str, str]) -> list[str]:
    rows = []
    for cmd in commands:
        name = cmd["command"]
        if name not in summaries:
            raise RenderError(f"no docstring summary for {name}")
        params = "; ".join(_cli_param(p) for p in cmd["params"]) or "-"
        rows.append((ids[name], f"| {ids[name]} | `{name}` | {params} | {summaries[name]} |"))
    return [row for _id, row in sorted(rows)]


# ---------------------------------------------------------------- MCP table


def _mcp_type(schema: dict[str, Any]) -> str:
    if "anyOf" in schema:
        parts = schema["anyOf"]
        non_null = [p for p in parts if p.get("type") != "null"]
        base = _mcp_type(non_null[0]) if len(non_null) == 1 else " | ".join(map(_mcp_type, non_null))
        return base + ("\\|null" if len(non_null) < len(parts) else "")
    if "$ref" in schema:
        return schema["$ref"].split("/")[-1]
    if "enum" in schema:
        return "{" + ",".join(schema["enum"]) + "}"
    if "pattern" in schema and schema["pattern"].startswith("^(") and schema["pattern"].endswith(")$"):
        return "{" + schema["pattern"][2:-2].replace("|", ",") + "}"
    kind = schema.get("type")
    if kind == "array":
        return f"list[{_mcp_type(schema['items'])}]"
    return str(kind)


def _mcp_params(tool: dict[str, Any], old_order: list[str]) -> str:
    schema = tool["input_schema"]
    props = schema.get("properties", {})
    required = set(schema.get("required", []))
    names = [n for n in old_order if n in props] + [n for n in props if n not in old_order]
    parts = []
    for name in names:
        prefix = "!" if name in required else ""
        parts.append(f"{prefix}{name}: {_mcp_type(props[name])}")
    return ", ".join(parts) if parts else "-"


def _mcp_rows(
    tools: list[dict[str, Any]], ids: dict[str, str], old_params: dict[str, list[str]]
) -> list[str]:
    rows = []
    for tool in tools:
        name = tool["name"]
        description = " ".join(tool["description"].split())
        params = _mcp_params(tool, old_params.get(name, []))
        rows.append((ids[name], f"| {ids[name]} | `{name}` | {params} | {description} |"))
    return [row for _id, row in sorted(rows)]


# ---------------------------------------------------------------- HTTP table


def _http_response(content_schema: dict[str, Any] | None) -> str:
    if not content_schema:
        return "any"
    if "$ref" in content_schema:
        return content_schema["$ref"].split("/")[-1]
    if "anyOf" in content_schema:
        parts = content_schema["anyOf"]
        non_null = [p for p in parts if p.get("type") != "null"]
        if len(non_null) == 1 and len(parts) == 2:
            return _http_response(non_null[0]) + "\\|null"
        return "any"
    if content_schema.get("type") == "object":
        return "object"
    if content_schema.get("type") == "array" and "items" in content_schema:
        return f"list[{_http_response(content_schema['items'])}]"
    return "any"


def _http_rows(openapi: dict[str, Any], ids: dict[tuple[str, str], str]) -> list[str]:
    ops: list[tuple[str, str, str, dict[str, Any]]] = []
    for path, item in openapi["paths"].items():
        for method, op in item.items():
            ops.append(((method.upper(), path), method, path, op))
    rows = []
    for key, _method, path, op in ops:
        codes = sorted(c for c in op["responses"] if c.startswith("2"))
        ok = codes[0] if codes else "-"
        query = []
        for param in op.get("parameters", []):
            if param["in"] == "query":
                query.append(param["name"] + ("!" if param.get("required") else ""))
        body = "-"
        if "requestBody" in op:
            schema = op["requestBody"]["content"]["application/json"]["schema"]
            body = _http_response(schema)
        response = "any"
        if ok in op["responses"] and "content" in op["responses"][ok]:
            schema = op["responses"][ok]["content"]["application/json"].get("schema")
            response = _http_response(schema)
        rows.append(
            (
                ids[key],
                f"| {ids[key]} | {key[0]} | `{path}` | {ok} | {','.join(query) or '-'} | {body} "
                f"| {response} | {op.get('summary', '')} |",
            )
        )
    return [row for _id, row in sorted(rows)]


# ---------------------------------------------------------------- database table


def _split_top_level(text: str) -> list[str]:
    parts, depth, current, quote = [], 0, "", None
    for ch in text:
        if quote:
            current += ch
            if ch == quote:
                quote = None
            continue
        if ch in "'\"":
            quote = ch
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append(current.strip())
            current = ""
            continue
        current += ch
    if current.strip():
        parts.append(current.strip())
    return parts


def _statements(sql_text: str) -> list[str]:
    statements, buf = [], ""
    for line in sql_text.splitlines():
        if not buf and (not line.strip() or line.startswith("--")):
            continue
        buf += line + "\n"
        if line.rstrip().endswith(";") and sqlite3.complete_statement(buf):
            statements.append(buf.strip())
            buf = ""
    return statements


def _db_rows(sql_text: str) -> list[str]:
    tables: dict[str, str] = {}
    columns: dict[str, str] = {}
    indexes: dict[str, int] = {}
    automatic = 0
    triggers = 0
    for stmt in _statements(sql_text):
        if m := re.match(r"CREATE VIRTUAL TABLE (\w+) USING \w+ ?\((.*)\);$", stmt, re.S):
            tables[m.group(1)] = m.group(1)
            columns[m.group(1)] = ", ".join(_split_top_level(m.group(2)))
        elif m := re.match(r"CREATE TABLE [\"']?(\w+)[\"']? ?\((.*?)\)(?: WITHOUT ROWID)?;$", stmt, re.S):
            name = m.group(1)
            cols = []
            for item in _split_top_level(m.group(2)):
                first = item.split()[0].strip("\"'`[]") if item.split() else ""
                if first.upper() in {"PRIMARY", "UNIQUE", "FOREIGN", "CHECK", "CONSTRAINT"}:
                    continue
                cols.append(first)
            tables[name] = name
            columns[name] = ", ".join(cols)
        elif m := re.match(r"CREATE (?:UNIQUE )?INDEX \S+ ON [\"']?(\w+)[\"']? ", stmt):
            indexes[m.group(1)] = indexes.get(m.group(1), 0) + 1
        elif stmt.startswith("CREATE TRIGGER"):
            triggers += 1
    automatic = len(re.findall(r"^-- automatic index ", sql_text, re.MULTILINE))
    if not tables:
        raise RenderError("no tables found in schema-0011.sql")
    rows = []
    for name in sorted(tables):
        rows.append(f"| `{name}` | {columns[name]} | {indexes.get(name, 0)} |")
    return rows, len(tables), sum(indexes.values()) + automatic, triggers


# ---------------------------------------------------------------- config table


def _config_type(schema: dict[str, Any]) -> str:
    if "anyOf" in schema:
        parts = schema["anyOf"]
        non_null = [p for p in parts if p.get("type") != "null"]
        return _config_type(non_null[0]) + "\\|null" if len(non_null) == 1 else "any"
    if schema.get("type") == "array":
        return f"array<{_config_type(schema.get('items', {}))}>"
    return str(schema.get("type", "any"))


def _config_rows(
    schema: dict[str, Any], old_order: dict[str, int], old_defaults: dict[str, str]
) -> list[tuple[str, str]]:
    defs = schema.get("$defs", {})
    rows: list[tuple[str, str]] = []

    def resolve(node: dict[str, Any]) -> dict[str, Any]:
        if "$ref" in node:
            return defs[node["$ref"].split("/")[-1]]
        return node

    def visit(prefix: str, node: dict[str, Any]) -> None:
        for name, raw in node.get("properties", {}).items():
            path = f"{prefix}{name}"
            if "additionalProperties" in raw:
                extra = raw["additionalProperties"]
                refs = [o["$ref"].split("/")[-1] for o in extra.get("oneOf", [])] or [
                    extra["$ref"].split("/")[-1]
                ]
                rows.append((f"{path}.<name>", "table: " + " or ".join(refs), "", ""))
                continue
            target = resolve(raw)
            if target.get("type") == "object" and "properties" in target:
                visit(path + ".", target)
                continue
            default = json.dumps(raw.get("default"), ensure_ascii=False) if "default" in raw else ""
            # default_factory fields carry no "default" in the snapshot; keep the committed value.
            default = default or old_defaults.get(path, "")
            allowed = ",".join(raw.get("enum", []))
            rows.append((path, _config_type(raw), default, allowed))

    visit("", schema)
    # Order: committed order where the key already appears there; new keys after, in snapshot order.
    indexed = list(enumerate(rows))
    indexed.sort(key=lambda item: old_order.get(item[1][0], len(old_order) + item[0]))
    return [row for _i, row in indexed]


# ---------------------------------------------------------------- assembly


def _check_prose(
    section: str,
    cli_count: int,
    top: int,
    tools: int,
    paths: int,
    ops: int,
    schemas: int,
    tables: int,
    indexes: int,
    triggers: int,
) -> None:
    expected = [
        f"**{cli_count} leaf commands** under {top} top-level names",
        f"**{tools} tools**",
        f"**{paths} paths, {ops} operations**",
        f"{schemas} component schemas",
        f"**{tables} tables**",
        f"**{indexes} indexes**",
        f"**{triggers} triggers**",
    ]
    for phrase in expected:
        if phrase not in section:
            raise RenderError(f"section 13 prose does not state the measured count: {phrase!r}")


def render(doc_text: str) -> str:
    cli = json.loads((CONTRACTS_DIR / "cli.json").read_text(encoding="utf-8"))
    mcp = json.loads((CONTRACTS_DIR / "mcp.json").read_text(encoding="utf-8"))
    openapi = json.loads((CONTRACTS_DIR / "openapi.json").read_text(encoding="utf-8"))
    config = json.loads((CONTRACTS_DIR / "config-schema.json").read_text(encoding="utf-8"))
    sql_text = (CONTRACTS_DIR / "schema-0011.sql").read_text(encoding="utf-8")

    section = _committed_section(doc_text)
    reg = _registry(section)
    summaries = cli_summaries(CLI_SRC_DIR)

    cli_names = [c["command"] for c in cli["commands"]]
    cli_ids = _assign_ids(cli_names, reg["cli"], "C", 3)
    mcp_names = [t["name"] for t in mcp["tools"]]
    mcp_ids = _assign_ids(mcp_names, reg["mcp"], "M", 2)
    http_keys = [(m.upper(), p) for p, item in openapi["paths"].items() for m in item]
    http_ids = _assign_ids(http_keys, reg["http"], "H", 3)

    db_rows, n_tables, n_indexes, n_triggers = _db_rows(sql_text)
    http_ops = len(http_keys)
    top = len(cli["top_level_names"])
    _check_prose(
        section,
        cli_count=len(cli_names),
        top=top,
        tools=len(mcp_names),
        paths=len(openapi["paths"]),
        ops=http_ops,
        schemas=len(openapi["components"]["schemas"]),
        tables=n_tables,
        indexes=n_indexes,
        triggers=n_triggers,
    )

    tables = {
        CLI_HEADER: _cli_rows(cli["commands"], summaries, cli_ids),
        MCP_HEADER: _mcp_rows(mcp["tools"], mcp_ids, reg["mcp_params"]),
        HTTP_HEADER: _http_rows(openapi, http_ids),
        DB_HEADER: db_rows,
        CONFIG_HEADER: [
            f"| `{k}` | {t} | {d} | {a} |"
            for k, t, d, a in _config_rows(config, reg["config"], reg["config_defaults"])
        ],
    }

    out: list[str] = []
    lines = section.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        if line in tables:
            out.append(line)
            out.append(lines[i + 1])  # separator row
            out.extend(tables[line])
            i += 2
            while i < len(lines) and lines[i].startswith("|"):
                i += 1
            continue
        out.append(line)
        i += 1
    body = "\n".join(out) + "\n"
    head = doc_text[: doc_text.find(SECTION_HEADING)]
    return head + body


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--check", action="store_true", help="exit 1 if section 13 would change")
    args = parser.parse_args(argv)
    doc_text = DOC_PATH.read_text(encoding="utf-8")
    rendered = render(doc_text)
    if args.check:
        if rendered != doc_text:
            print(f"{DOC_PATH} section 13 is out of date; run tools/contracts/render_md.py", file=sys.stderr)
            return 1
        return 0
    if rendered != doc_text:
        DOC_PATH.write_bytes(rendered.encode("utf-8"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
