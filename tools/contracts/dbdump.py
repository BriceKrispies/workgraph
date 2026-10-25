"""Per-revision schema snapshots, fixture databases and the canonical database dump (task CF-05).

Usage::

    uv run python tools/contracts/dbdump.py build            # rewrite contracts/schema-NNNN.sql and golden/db/rev-NNNN.db
    uv run python tools/contracts/dbdump.py dump DB [-o FILE]  # canonical dump of any database (section 3.1 item 4)

``build`` migrates a fresh database to each revision in ``REVISIONS`` with the Python migration
chain, writes its ``sqlite_master`` to ``contracts/schema-NNNN.sql``, then fills every table that
exists at that revision with ``ROWS_PER_TABLE`` deterministic rows (raw SQL, foreign keys on) and
stores the file as ``golden/db/rev-NNNN.db``. The Rust side migrates each fixture to head and must
produce the same dump as Python does for the same file.

To add a revision, append it to ``REVISIONS`` and run ``build``.

Table constraints are normalised with ``dump._canonical_create_table``, the same function that
writes ``contracts/schema-0010.sql``, so the unstable constraint order of ``subagent_runs`` does
not show up in either form.

Every output is UTF-8 with ``\\n`` line endings and sorted keys, so two runs give identical bytes.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from dump import _canonical_create_table

ROOT = Path(__file__).resolve().parents[2]
CONTRACTS_DIR = ROOT / "contracts"
GOLDEN_DB_DIR = ROOT / "golden" / "db"

REVISIONS = ("0001", "0002", "0003", "0004", "0005", "0006", "0007", "0008", "0009", "0010")
HEAD = REVISIONS[-1]
ROWS_PER_TABLE = 2

EXCLUDED_TABLES = frozenset({"alembic_version"})
EXCLUDED_PREFIXES = ("task_fts",)  # the FTS5 table and its shadow tables

# Columns that do not exist at head (dropped by 0009) but hold JSON that a data migration parses.
EXTRA_JSON_COLUMNS = frozenset({("tasks", "external_refs")})


def _is_excluded(name: str) -> bool:
    return name in EXCLUDED_TABLES or name.startswith(EXCLUDED_PREFIXES)


def _quote(identifier: str) -> str:
    return '"' + identifier.replace('"', '""') + '"'


# ---------------------------------------------------------------- canonical dump


def _normalised_sql(kind: str, sql: str | None) -> str | None:
    if sql is None:
        return None
    if kind == "table":
        return _canonical_create_table(sql)
    return sql.rstrip().rstrip(";")


def _canonical_value(value: Any) -> Any:
    if isinstance(value, bytes):
        return {"blob": value.hex()}
    if isinstance(value, str) and value.lstrip()[:1] in ("{", "["):
        # JSON columns are stored as TEXT; parse them so that key order and whitespace do not matter.
        try:
            return json.loads(value)
        except ValueError:
            return value
    return value


def _table_document(conn: sqlite3.Connection, table: str) -> dict[str, Any]:
    info = conn.execute(f"PRAGMA table_info({_quote(table)})").fetchall()
    names = [column[1] for column in info]
    pk_columns = [column[1] for column in sorted((c for c in info if c[5]), key=lambda c: c[5])]
    order = ", ".join(_quote(c) for c in pk_columns) or "rowid"
    select = ", ".join(_quote(n) for n in names)
    cursor = conn.execute(f"SELECT {select} FROM {_quote(table)} ORDER BY {order}")
    rows = [{n: _canonical_value(v) for n, v in zip(names, row, strict=True)} for row in cursor]
    return {"columns": names, "rows": rows}


def canonical_document(database: Path) -> dict[str, Any]:
    """Return the canonical dump of ``database`` as plain data (section 3.1 item 4)."""
    conn = sqlite3.connect(str(database))
    try:
        schema = [
            {"type": kind, "name": name, "table": table, "sql": _normalised_sql(kind, sql)}
            for kind, name, table, sql in conn.execute(
                "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name"
            )
        ]
        table_names = [
            name
            for (name,) in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            if not _is_excluded(name)
        ]
        tables = {name: _table_document(conn, name) for name in table_names}
    finally:
        conn.close()
    return {"schema": schema, "tables": tables}


def canonical_dump(database: Path) -> bytes:
    text = json.dumps(canonical_document(database), sort_keys=True, indent=1, ensure_ascii=False)
    return (text + "\n").encode("utf-8")


# ---------------------------------------------------------------- schema per revision


def _schema_text(database: Path, revision: str) -> str:
    """``sqlite_master`` in the layout ``dump._schema_snapshot`` uses for ``schema-0010.sql``."""
    state = "head" if revision == HEAD else "revision"
    lines = [
        f"-- sqlite_master of a database migrated to {state} {revision}.",
        "-- Automatic indexes have no SQL text and are listed as comments.",
        "",
    ]
    conn = sqlite3.connect(str(database))
    try:
        rows = conn.execute(
            "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name"
        ).fetchall()
    finally:
        conn.close()
    for kind, name, tbl_name, sql in rows:
        if sql is None:
            lines.append(f"-- automatic {kind} {name} on {tbl_name}")
        elif kind == "table":
            lines.append(_canonical_create_table(sql) + ";")
        else:
            lines.append(sql.rstrip().rstrip(";") + ";")
        lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------- fixture population


def _column_kinds() -> dict[tuple[str, str], str]:
    """Map (table, column) to ``json`` or ``datetime`` from the head ORM types."""
    from workgraph.persistence.orm import Base
    from workgraph.persistence.types import JSONText, UTCDateTime

    kinds = dict.fromkeys(EXTRA_JSON_COLUMNS, "json")
    for table in Base.metadata.tables.values():
        for column in table.columns:
            if isinstance(column.type, JSONText):
                kinds[(table.name, column.name)] = "json"
            elif isinstance(column.type, UTCDateTime):
                kinds[(table.name, column.name)] = "datetime"
    return kinds


def _json_sample(table: str, column: str, index: int) -> Any:
    if (table, column) == ("tasks", "external_refs"):
        # Migration 0009 turns each of these into a task link; give it real input.
        return [{"system": "github", "id": str(index), "url": f"https://github.example/{index}"}]
    return []


def _sample(kinds: dict[tuple[str, str], str], table: str, column: str, declared: str, index: int) -> Any:
    kind = kinds.get((table, column))
    if kind == "json":
        return json.dumps(_json_sample(table, column, index), sort_keys=True, separators=(",", ":"))
    if kind == "datetime":
        return f"2026-01-0{index}T00:00:00.000000+00:00"
    upper = declared.upper()
    if "INT" in upper or "BOOL" in upper or "NUM" in upper:
        return index
    if any(part in upper for part in ("REAL", "FLOA", "DOUB")):
        return index + 0.5
    if "BLOB" in upper:
        return bytes([index])
    return f"{table}.{column}.{index}"


def _table_names(conn: sqlite3.Connection) -> list[str]:
    names = [
        name
        for (name,) in conn.execute(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite%' ORDER BY name"
        )
    ]
    return [name for name in names if not _is_excluded(name)]


def _pk_columns(conn: sqlite3.Connection, table: str) -> list[str]:
    info = conn.execute(f"PRAGMA table_info({_quote(table)})").fetchall()
    return [column[1] for column in sorted((c for c in info if c[5]), key=lambda c: c[5])]


def _foreign_keys(conn: sqlite3.Connection, table: str) -> list[tuple[str, list[str], list[str], bool]]:
    """Each foreign key as (parent table, child columns, parent columns, all child columns nullable)."""
    rows = conn.execute(f"PRAGMA foreign_key_list({_quote(table)})").fetchall()
    grouped: dict[int, tuple[str, list[str], list[str]]] = {}
    for fk_id, _seq, parent, from_col, to_col, *_ in sorted(rows, key=lambda r: (r[0], r[1])):
        entry = grouped.setdefault(fk_id, (parent, [], []))
        entry[1].append(from_col)
        entry[2].append(to_col)
    info = {c[1]: c for c in conn.execute(f"PRAGMA table_info({_quote(table)})").fetchall()}
    result = []
    for parent, from_cols, to_cols in (grouped[k] for k in sorted(grouped)):
        parent_cols = to_cols if all(c is not None for c in to_cols) else _pk_columns(conn, parent)
        nullable = all(info[c][3] == 0 for c in from_cols)
        result.append((parent, from_cols, parent_cols, nullable))
    return result


def _insert_order(conn: sqlite3.Connection, tables: list[str]) -> list[str]:
    deps = {
        table: {parent for parent, *_ in _foreign_keys(conn, table) if parent != table} for table in tables
    }
    for table, parents in deps.items():
        missing = parents - set(tables)
        if missing:
            raise RuntimeError(f"{table} references tables that do not exist here: {sorted(missing)}")
    order: list[str] = []
    placed: set[str] = set()
    while len(order) < len(tables):
        ready = sorted(t for t in tables if t not in placed and deps[t] <= placed)
        if not ready:
            raise RuntimeError(f"foreign key cycle among {sorted(set(tables) - placed)}")
        order.extend(ready)
        placed.update(ready)
    return order


def _insert_row(
    conn: sqlite3.Connection, table: str, row_index: int, kinds: dict[tuple[str, str], str]
) -> None:
    values: dict[str, Any] = {}
    handled: set[str] = set()
    for group, (parent, from_cols, parent_cols, nullable) in enumerate(_foreign_keys(conn, table)):
        handled.update(from_cols)
        if parent == table and nullable:
            continue  # self reference: leave NULL so that no row points at another row of this table
        if parent == table:
            raise RuntimeError(f"{table}: non-nullable self reference cannot be populated")
        # Distinct groups point at distinct parent rows, so CHECK (a <> b) across two keys holds.
        offset = (row_index + group) % ROWS_PER_TABLE
        selected = ", ".join(_quote(c) for c in parent_cols)
        ref = conn.execute(
            f"SELECT {selected} FROM {_quote(parent)} ORDER BY rowid LIMIT 1 OFFSET ?", (offset,)
        ).fetchone()
        values.update(zip(from_cols, ref, strict=True))
    info = conn.execute(f"PRAGMA table_info({_quote(table)})").fetchall()
    pk_columns = [c[1] for c in info if c[5]]
    for _cid, name, declared, _notnull, _default, pk in info:
        if name in handled:
            continue
        if pk and len(pk_columns) == 1 and declared.upper() == "INTEGER":
            continue  # rowid alias: assigned by SQLite
        values[name] = _sample(kinds, table, name, declared, row_index + 1)
    if values:
        names = ", ".join(_quote(c) for c in values)
        marks = ", ".join("?" for _ in values)
        conn.execute(f"INSERT INTO {_quote(table)} ({names}) VALUES ({marks})", tuple(values.values()))
    else:
        conn.execute(f"INSERT INTO {_quote(table)} DEFAULT VALUES")


def populate(database: Path, kinds: dict[tuple[str, str], str]) -> None:
    conn = sqlite3.connect(str(database))
    try:
        conn.execute("PRAGMA foreign_keys = ON")
        tables = _table_names(conn)
        for table in _insert_order(conn, tables):
            for row_index in range(ROWS_PER_TABLE):
                _insert_row(conn, table, row_index, kinds)
        conn.commit()
    finally:
        conn.close()


# ---------------------------------------------------------------- build


def _remove_database(path: Path) -> None:
    for suffix in ("", "-wal", "-shm", "-journal"):
        candidate = Path(str(path) + suffix)
        if candidate.exists():
            candidate.unlink()


def _single_file(database: Path) -> None:
    """Leave the fixture as one file in rollback-journal mode, vacuumed, so its bytes are stable."""
    conn = sqlite3.connect(str(database))
    try:
        conn.execute("PRAGMA journal_mode = DELETE").fetchone()
        conn.execute("VACUUM")
    finally:
        conn.close()


def build() -> None:
    from workgraph.persistence.db import Database
    from workgraph.persistence.migrate import upgrade

    kinds = _column_kinds()
    CONTRACTS_DIR.mkdir(parents=True, exist_ok=True)
    GOLDEN_DB_DIR.mkdir(parents=True, exist_ok=True)
    for revision in REVISIONS:
        path = GOLDEN_DB_DIR / f"rev-{revision}.db"
        _remove_database(path)
        db = Database(path)
        try:
            upgrade(db, revision)
        finally:
            db.engine.dispose()
        schema = _schema_text(path, revision)
        (CONTRACTS_DIR / f"schema-{revision}.sql").write_bytes(schema.encode("utf-8"))
        populate(path, kinds)
        _single_file(path)
        print(f"revision {revision}: {path.relative_to(ROOT).as_posix()} ({path.stat().st_size} bytes)")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("build", help="rewrite contracts/schema-NNNN.sql and golden/db/rev-NNNN.db")
    dump_parser = sub.add_parser("dump", help="print the canonical dump of a database")
    dump_parser.add_argument("database", type=Path)
    dump_parser.add_argument("-o", "--output", type=Path, help="write to FILE instead of stdout")
    args = parser.parse_args(argv)
    if args.command == "build":
        build()
        return 0
    data = canonical_dump(args.database)
    if args.output is not None:
        args.output.write_bytes(data)
    else:
        sys.stdout.buffer.write(data)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
