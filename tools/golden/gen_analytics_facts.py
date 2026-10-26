"""Golden vectors GV-15, fact tables: what ``analytics/warehouse.py`` builds from the source rows.

Reads ``golden/analytics/input-<name>.json`` (written by ``gen_analytics.py``), copies its tables
into a SQLite file with the warehouse's own ``Warehouse`` loader and dumps the fact tables the
warehouse builds in DuckDB. Rows are sorted by their JSON text, timestamps are Python
``isoformat()`` strings, and DuckDB runs single-threaded, as in ``gen_analytics.py``.

Run after ``gen_analytics.py``: ``uv run python tools/golden/gen_analytics_facts.py``
"""

from __future__ import annotations

import json
import sqlite3
import sys
import tempfile
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "src"
GOLDEN = ROOT / "golden" / "analytics"
FACT_TABLES = ("task_commits", "task_code", "task_files", "task_surface", "task_facts", "attempt_facts")

sys.path.insert(0, str(SRC))

import duckdb  # noqa: E402

_duckdb_connect = duckdb.connect


def _single_threaded_connect(*args: Any, **kwargs: Any) -> Any:
    # Row numbering over ties is not fixed when DuckDB runs several threads; one thread fixes it.
    kwargs["config"] = {"threads": 1, **kwargs.get("config", {})}
    return _duckdb_connect(*args, **kwargs)


duckdb.connect = _single_threaded_connect  # type: ignore[assignment]

from workgraph.analytics.warehouse import Warehouse  # noqa: E402


def load_source(source_file: Path, db_file: Path) -> None:
    """Copies the golden tables into SQLite. Column types are left open, so each value keeps the
    type the golden file recorded, as the database held it."""
    source = json.loads(source_file.read_text(encoding="utf-8"))
    con = sqlite3.connect(db_file)
    try:
        for name, table in source["tables"].items():
            columns = table["columns"]
            con.execute(f"CREATE TABLE {name} ({', '.join(columns)})")
            con.executemany(
                f"INSERT INTO {name} VALUES ({', '.join('?' for _ in columns)})",
                table["rows"],
            )
        con.commit()
    finally:
        con.close()


def plain(value: Any) -> Any:
    if isinstance(value, datetime):
        return value.replace(tzinfo=UTC).isoformat()
    return value


def dump(name: str) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        db_file = Path(tmp) / "source.db"
        load_source(GOLDEN / f"input-{name}.json", db_file)
        warehouse = Warehouse(db_file)
        try:
            con = warehouse.connection(force=True)
            tables: dict[str, Any] = {}
            for table in FACT_TABLES:
                cursor = con.execute(f"SELECT * FROM {table}")
                columns = [d[0] for d in cursor.description or []]
                rows = [
                    {column: plain(value) for column, value in zip(columns, row, strict=True)}
                    for row in cursor.fetchall()
                ]
                tables[table] = sorted(rows, key=lambda row: json.dumps(row, sort_keys=True))
        finally:
            warehouse.close()
    text = json.dumps({"database": name, "tables": tables}, sort_keys=True, indent=1, ensure_ascii=False)
    (GOLDEN / f"facts-{name}.json").write_text(text + "\n", encoding="utf-8")
    counts = ", ".join(f"{table} {len(tables[table])}" for table in FACT_TABLES)
    print(f"facts-{name}.json: {counts}")


def main() -> None:
    for name in ("demo", "gen"):
        dump(name)


if __name__ == "__main__":
    main()
