"""Extract the Python contract snapshots that the Rust rewrite must match (task CF-01).

Writes five files under ``contracts/``:

* ``cli.json``           the Typer/Click command tree: every leaf command with its arguments and
                         options (names, types, defaults, env vars, choices). Help text is left out
                         because it is not a contract (decision 1 in ``docs/rust-rewrite.md`` section 11).
* ``mcp.json``           the MCP server's ``tools/list``, ``resources/list`` and
                         ``resources/templates/list`` as the SDK reports them for a server built by
                         ``workgraph.mcp.server.build_server``.
* ``openapi.json``       the FastAPI OpenAPI document of ``workgraph.api.app.create_app``.
* ``config-schema.json`` the JSON Schema of ``workgraph.config.settings.GlobalConfig``.
* ``schema-0011.sql``    ``sqlite_master`` of a database migrated to head ``0011``.

Usage::

    uv run python tools/contracts/dump.py           # rewrite the snapshots
    uv run python tools/contracts/dump.py --check   # exit 1 if any snapshot differs

Every file is written as sorted-key JSON (or SQL text) with ``\\n`` line endings and UTF-8, so two
runs give identical bytes. The drift test in ``tests/unit/test_contract_snapshots.py`` calls
``build_snapshots`` directly and compares the result with the committed files.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import re
import sqlite3
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
CONTRACTS_DIR = ROOT / "contracts"

SNAPSHOT_NAMES = ("cli.json", "mcp.json", "openapi.json", "config-schema.json", "schema-0011.sql")


def _json_bytes(data: Any) -> bytes:
    text = json.dumps(data, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    return text.encode("utf-8")


def _jsonable(value: Any) -> Any:
    """Render a Click default or choice as JSON without leaking object reprs."""
    if value is None or isinstance(value, bool | int | float | str):
        return value
    if isinstance(value, list | tuple | set | frozenset):
        items = [_jsonable(v) for v in value]
        return (
            sorted(items, key=lambda v: json.dumps(v, sort_keys=True))
            if isinstance(value, set | frozenset)
            else items
        )
    if isinstance(value, dict):
        return {str(k): _jsonable(v) for k, v in value.items()}
    if callable(value):
        return "<callable>"
    if isinstance(value, Path):
        return str(value)
    return repr(value)


# ---------------------------------------------------------------- CLI


def _param_default(param: Any) -> Any:
    default = param.default
    if default is None:
        return None
    # Click marks "no default" with a sentinel-like object; treat anything non-plain as absent.
    if callable(default) or type(default).__name__ in {"Sentinel", "UNSET"}:
        return "<callable>" if callable(default) else None
    return _jsonable(default)


def _param_record(param: Any) -> dict[str, Any]:
    # Typer vendors Click as ``typer._click``, so ``isinstance`` checks against ``click`` are False.
    type_name = getattr(param.type, "name", type(param.type).__name__)
    choices = getattr(param.type, "choices", None)
    if param.param_type_name == "argument":
        return {
            "kind": "argument",
            "name": param.name,
            "type": type_name,
            "required": bool(param.required),
            "nargs": param.nargs,
            "default": _param_default(param),
        }
    return {
        "kind": "option",
        "opts": sorted(param.opts),
        "secondary_opts": sorted(param.secondary_opts),
        "type": type_name,
        "choices": _jsonable(choices) if choices is not None else None,
        "is_flag": bool(param.is_flag),
        "multiple": bool(param.multiple),
        "required": bool(param.required),
        "default": _param_default(param),
        "envvar": _jsonable(param.envvar),
    }


def _walk_cli(cmd: Any, path: tuple[str, ...], leaves: list[dict[str, Any]]) -> None:
    if getattr(cmd, "commands", None):
        for name in sorted(cmd.commands):
            _walk_cli(cmd.commands[name], (*path, name), leaves)
        return
    params = [p for p in cmd.params if p.name != "help"]
    leaves.append(
        {
            "command": " ".join(("workgraph", *path)),
            "params": [_param_record(p) for p in params],
        }
    )


def cli_snapshot() -> dict[str, Any]:
    import typer.main

    from workgraph.cli.main import app

    root = typer.main.get_command(app)
    leaves: list[dict[str, Any]] = []
    _walk_cli(root, (), leaves)
    leaves.sort(key=lambda leaf: leaf["command"])
    root_options = [_param_record(p) for p in root.params if p.name not in {"help", "version"}]
    top_level = sorted({leaf["command"].split(" ")[1] for leaf in leaves if " " in leaf["command"]})
    return {
        "root_options": sorted(root_options, key=lambda r: r.get("opts", [r.get("name")])[0]),
        "leaf_count": len(leaves),
        "top_level_names": top_level,
        "commands": leaves,
    }


# ---------------------------------------------------------------- MCP


def _tool_record(tool: Any) -> dict[str, Any]:
    return tool.model_dump(mode="json", exclude_none=True)


async def _mcp_snapshot(app: Any) -> dict[str, Any]:
    from workgraph.mcp.server import build_server

    server = build_server(app, agent_name="contract-snapshot")
    tools = await server.list_tools()
    resources = await server.list_resources()
    templates = await server.list_resource_templates()
    return {
        "tools": sorted((_tool_record(t) for t in tools), key=lambda t: t["name"]),
        "resources": sorted((_tool_record(r) for r in resources), key=lambda r: str(r["uri"])),
        "resource_templates": sorted(
            (_tool_record(t) for t in templates), key=lambda t: str(t["uri_template"])
        ),
    }


# ---------------------------------------------------------------- OpenAPI


def _openapi_snapshot(app: Any) -> dict[str, Any]:
    from workgraph.api.app import create_app

    asgi = create_app(app, with_mcp=False)
    # create_app wraps the FastAPI application in the security middleware; the schema lives on it.
    api = getattr(asgi, "app", asgi)
    return api.openapi()


# ---------------------------------------------------------------- config


def _config_snapshot() -> dict[str, Any]:
    from pydantic.json_schema import GenerateJsonSchema

    from workgraph.config.settings import GlobalConfig

    class _FactoryDefaults(GenerateJsonSchema):
        """Also emit defaults produced by ``default_factory``, which pydantic leaves out of the schema.

        The factory is called once at schema time, so the default is what a fresh config holds.
        """

        def get_default_value(self, schema: Any) -> Any:
            factory = schema.get("default_factory")
            if factory is not None:
                return factory()
            return super().get_default_value(schema)

    return GlobalConfig.model_json_schema(schema_generator=_FactoryDefaults)


# ---------------------------------------------------------------- database


def _schema_snapshot(database_file: Path) -> bytes:
    conn = sqlite3.connect(str(database_file))
    try:
        rows = conn.execute(
            "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name"
        ).fetchall()
    finally:
        conn.close()
    lines = [
        "-- sqlite_master of a database migrated to head 0011.",
        "-- Automatic indexes have no SQL text and are listed as comments.",
        "",
    ]
    for kind, name, tbl_name, sql in rows:
        if sql is None:
            lines.append(f"-- automatic {kind} {name} on {tbl_name}")
        elif kind == "table":
            lines.append(_canonical_create_table(sql) + ";")
        else:
            lines.append(sql.rstrip().rstrip(";") + ";")
        lines.append("")
    return ("\n".join(lines)).encode("utf-8")


_CONSTRAINT_START = re.compile(r"(CONSTRAINT|PRIMARY|UNIQUE|FOREIGN|CHECK)\b", re.IGNORECASE)


def _split_top_level(body: str) -> list[str]:
    """Split a CREATE TABLE body at commas that sit outside parentheses and string literals."""
    items: list[str] = []
    depth = 0
    quote = ""
    current: list[str] = []
    for ch in body:
        if quote:
            if ch == quote:
                quote = ""
        elif ch in "'\"":
            quote = ch
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        elif ch == "," and depth == 0:
            items.append("".join(current))
            current = []
            continue
        current.append(ch)
    items.append("".join(current))
    return [" ".join(item.split()) for item in items if item.strip()]


def _canonical_create_table(sql: str) -> str:
    """Put a table's definition in a fixed form.

    The migration chain adds table constraints in an order that is not stable from one run to the
    next (``subagent_runs`` flips between two orders). Column order is part of the table, so columns
    keep their place; table constraints are sorted and placed after the columns. SQLite treats both
    orders as the same table, and the Rust side is compared against this form.
    """
    head, sep, rest = sql.partition("(")
    close = rest.rfind(")")
    if not sep or close < 0:
        return sql
    body, tail = rest[:close], rest[close + 1 :]
    columns: list[str] = []
    constraints: list[str] = []
    for item in _split_top_level(body):
        (constraints if _CONSTRAINT_START.match(item) else columns).append(item)
    parts = columns + sorted(constraints)
    return f"{head.rstrip()} (\n\t" + ",\n\t".join(parts) + f"\n){tail.rstrip()}"


# ---------------------------------------------------------------- driver


def build_snapshots() -> dict[str, bytes]:
    """Return the bytes of every snapshot file, keyed by file name.

    Runs against a throwaway Workgraph home, so the user's own data directory is never touched.
    """
    import os

    from workgraph.app import Workgraph
    from workgraph.config.paths import AppPaths

    with tempfile.TemporaryDirectory(prefix="wg-contracts-") as tmp:
        home = Path(tmp) / "home"
        previous = os.environ.get("WORKGRAPH_HOME")
        os.environ["WORKGRAPH_HOME"] = str(home)
        try:
            paths = AppPaths.resolve(home)
            app = Workgraph(paths)
            try:
                mcp = asyncio.run(_mcp_snapshot(app))
                openapi = _openapi_snapshot(app)
                schema_sql = _schema_snapshot(paths.database_file)
            finally:
                app.close()
        finally:
            if previous is None:
                os.environ.pop("WORKGRAPH_HOME", None)
            else:
                os.environ["WORKGRAPH_HOME"] = previous
        return {
            "cli.json": _json_bytes(cli_snapshot()),
            "mcp.json": _json_bytes(mcp),
            "openapi.json": _json_bytes(openapi),
            "config-schema.json": _json_bytes(_config_snapshot()),
            "schema-0011.sql": schema_sql,
        }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument(
        "--check", action="store_true", help="compare with the committed snapshots instead of writing them"
    )
    args = parser.parse_args(argv)
    snapshots = build_snapshots()
    if args.check:
        stale = [
            name
            for name, data in snapshots.items()
            if not (CONTRACTS_DIR / name).is_file() or (CONTRACTS_DIR / name).read_bytes() != data
        ]
        if stale:
            print("contract snapshots differ: " + ", ".join(stale), file=sys.stderr)
            print("run: uv run python tools/contracts/dump.py", file=sys.stderr)
            return 1
        print("contract snapshots match")
        return 0
    CONTRACTS_DIR.mkdir(parents=True, exist_ok=True)
    for name, data in snapshots.items():
        (CONTRACTS_DIR / name).write_bytes(data)
        print(f"wrote {CONTRACTS_DIR / name} ({len(data)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
