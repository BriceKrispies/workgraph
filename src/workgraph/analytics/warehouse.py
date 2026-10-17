"""Read-only analytical snapshot of the operational database in DuckDB.

SQLite stays authoritative. The warehouse copies the tables analytics needs into an
in-process DuckDB database through a plain read-only SQLite connection (no DuckDB
extensions, so nothing is downloaded), then builds typed views and fact tables:

* ``task_facts``      - one row per task with lifecycle timestamps and rollups
* ``attempt_facts``   - one row per attempt (top-level and nested)
* ``task_commits``    - distinct (task, commit) pairs with confidence >= 0.5,
                        de-duplicated across rewrites/cherry-picks by patch id
* ``commit_facts``    - commits with file-class breakdowns

The snapshot is rebuilt whenever the event log or commit table has advanced, so a
query never observes partially-copied state and never writes to SQLite.
"""

from __future__ import annotations

import sqlite3
import threading
from pathlib import Path
from typing import Any

import duckdb
import pyarrow as pa

TABLES: dict[str, list[str] | None] = {
    "workspaces": ["id", "slug", "name"],
    "projects": ["id", "workspace_id", "slug", "name", "status"],
    "repositories": ["id", "workspace_id", "name", "access_mode", "archived_at"],
    "repository_dependencies": ["from_repository_id", "to_repository_id", "kind", "description"],
    "project_repositories": ["project_id", "repository_id"],
    "tasks": [
        "id",
        "workspace_id",
        "number",
        "key",
        "title",
        "status",
        "priority",
        "category",
        "parent_id",
        "assignee",
        "assigned_agent",
        "created_at",
        "updated_at",
        "started_at",
        "completed_at",
        "accepted_at",
        "archived_at",
        "auto_execute",
    ],
    "task_projects": ["task_id", "project_id"],
    "task_repositories": ["task_id", "repository_id", "access"],
    "task_tags": ["task_id", "tag"],
    "task_dependencies": ["task_id", "depends_on_id", "created_at"],
    "task_relations": ["source_task_id", "target_task_id", "relation_type", "created_at"],
    "agent_attempts": [
        "id",
        "workspace_id",
        "task_id",
        "number",
        "status",
        "provider",
        "adapter",
        "model",
        "parent_attempt_id",
        "retry_of_attempt_id",
        "rerun_of_attempt_id",
        "created_at",
        "queued_at",
        "started_at",
        "finished_at",
        "queue_wait_ms",
        "duration_ms",
        "exit_reason",
        "usage",
        "usage_source",
        "cost_usd",
        "cost_source",
        "modified_repository_ids",
    ],
    "commits": [
        "id",
        "repository_id",
        "sha",
        "author_name",
        "author_email",
        "committed_at",
        "subject",
        "is_merge",
        "patch_id",
        "insertions",
        "deletions",
        "files_changed",
        "reachable",
    ],
    "commit_file_changes": [
        "id",
        "commit_id",
        "path",
        "old_path",
        "change_type",
        "insertions",
        "deletions",
        "is_binary",
        "language",
        "file_class",
        "module",
    ],
    "commit_task_links": ["commit_id", "task_id", "source", "confidence", "attempt_id"],
    "commit_analyses": [
        "commit_id",
        "functions_affected",
        "types_affected",
        "files_parsed",
        "files_fallback",
    ],
    "commit_symbols": ["file_change_id", "symbol_kind", "name", "change_kind", "lines_changed"],
    "verification_runs": [
        "id",
        "workspace_id",
        "task_id",
        "attempt_id",
        "status",
        "required_passed",
        "started_at",
        "finished_at",
        "created_at",
    ],
    "verification_check_results": [
        "id",
        "run_id",
        "name",
        "kind",
        "required",
        "repository_id",
        "status",
        "exit_code",
        "duration_ms",
        "timed_out",
        "failure_signatures",
        "created_at",
    ],
    "task_acceptances": ["task_id", "attempt_id", "decision", "mode", "created_at"],
    "task_change_surfaces": [
        "task_id",
        "repository_id",
        "files_changed",
        "insertions",
        "deletions",
        "renames",
        "binary_files",
        "functions_affected",
        "types_affected",
    ],
    "change_sets": ["id", "task_id", "attempt_id", "status"],
    "change_set_items": ["change_set_id", "repository_id", "status"],
    "classifications": [
        "id",
        "task_id",
        "source",
        "status",
        "provider",
        "model",
        "category",
        "confidence",
        "is_current",
        "created_at",
    ],
}
EVENT_TYPES = (
    "task.status_changed",
    "task.reopened",
    "task.completed",
    "task.accepted",
    "task.escalated",
    "task.rejected",
)
TIMESTAMP_SUFFIXES = ("_at",)


def _sqlite_columns(conn: sqlite3.Connection, table: str) -> list[str]:
    return [row[1] for row in conn.execute(f"PRAGMA table_info({table})")]


def _arrow_table(cursor: sqlite3.Cursor, columns: list[str]) -> pa.Table:
    rows = cursor.fetchall()
    data = {col: [row[i] for row in rows] for i, col in enumerate(columns)}
    arrays = {}
    for col, values in data.items():
        if all(v is None or isinstance(v, int) for v in values) and any(v is not None for v in values):
            arrays[col] = pa.array(values, type=pa.int64())
        elif all(v is None or isinstance(v, int | float) for v in values) and any(
            isinstance(v, float) for v in values
        ):
            arrays[col] = pa.array([None if v is None else float(v) for v in values], type=pa.float64())
        else:
            arrays[col] = pa.array([None if v is None else str(v) for v in values], type=pa.string())
    if not rows:
        return pa.table({col: pa.array([], type=pa.string()) for col in columns})
    return pa.table(arrays)


class Warehouse:
    def __init__(self, db_path: Path) -> None:
        self.db_path = db_path
        self._lock = threading.RLock()
        self._con: duckdb.DuckDBPyConnection | None = None
        self._stamp: tuple[int, int, int] | None = None

    def _source_stamp(self, conn: sqlite3.Connection) -> tuple[int, int, int]:
        seq = conn.execute("SELECT COALESCE(MAX(seq), 0) FROM events").fetchone()[0]
        commits = conn.execute("SELECT COUNT(*) FROM commits").fetchone()[0]
        surfaces = conn.execute("SELECT COUNT(*) FROM task_change_surfaces").fetchone()[0]
        return int(seq), int(commits), int(surfaces)

    def connection(self, *, force: bool = False) -> duckdb.DuckDBPyConnection:
        with self._lock:
            uri = f"file:{self.db_path.as_posix()}?mode=ro"
            source = sqlite3.connect(uri, uri=True, timeout=15)
            try:
                stamp = self._source_stamp(source)
                if self._con is not None and not force and stamp == self._stamp:
                    return self._con
                con = duckdb.connect(":memory:")
                self._load(source, con)
                self._build(con)
            finally:
                source.close()
            if self._con is not None:
                self._con.close()
            self._con, self._stamp = con, stamp
            return con

    def _load(self, source: sqlite3.Connection, con: duckdb.DuckDBPyConnection) -> None:
        for table, wanted in TABLES.items():
            available = _sqlite_columns(source, table)
            columns = [c for c in (wanted or available) if c in available]
            cursor = source.execute(f"SELECT {', '.join(columns)} FROM {table}")
            arrow = _arrow_table(cursor, columns)
            con.register("_src", arrow)
            casts = []
            for col in columns:
                if col.endswith(TIMESTAMP_SUFFIXES):
                    casts.append(
                        f"TRY_CAST(REPLACE(CAST({col} AS VARCHAR), '+00:00', '') AS TIMESTAMP) AS {col}"
                    )
                elif col in (
                    "is_merge",
                    "reachable",
                    "is_binary",
                    "required",
                    "required_passed",
                    "timed_out",
                    "is_current",
                    "auto_execute",
                ):
                    casts.append(f"CAST({col} AS BOOLEAN) AS {col}")
                elif col in (
                    "insertions",
                    "deletions",
                    "files_changed",
                    "duration_ms",
                    "queue_wait_ms",
                    "number",
                    "functions_affected",
                    "types_affected",
                    "renames",
                    "binary_files",
                    "exit_code",
                    "files_parsed",
                    "files_fallback",
                    "lines_changed",
                ):
                    casts.append(f"CAST({col} AS BIGINT) AS {col}")
                elif col in ("confidence", "cost_usd"):
                    casts.append(f"CAST({col} AS DOUBLE) AS {col}")
                else:
                    casts.append(f"CAST({col} AS VARCHAR) AS {col}")
            con.execute(f"CREATE TABLE {table} AS SELECT {', '.join(casts)} FROM _src")
            con.unregister("_src")
        placeholders = ",".join("?" for _ in EVENT_TYPES)
        cursor = source.execute(
            f"SELECT seq, workspace_id, event_type, entity_id, actor, occurred_at, data FROM events "
            f"WHERE event_type IN ({placeholders})",
            EVENT_TYPES,
        )
        arrow = _arrow_table(
            cursor, ["seq", "workspace_id", "event_type", "entity_id", "actor", "occurred_at", "data"]
        )
        con.register("_src", arrow)
        con.execute(
            "CREATE TABLE events AS SELECT CAST(seq AS BIGINT) AS seq, CAST(workspace_id AS VARCHAR) AS workspace_id, "
            "CAST(event_type AS VARCHAR) AS event_type, CAST(entity_id AS VARCHAR) AS entity_id, "
            "CAST(actor AS VARCHAR) AS actor, "
            "TRY_CAST(REPLACE(CAST(occurred_at AS VARCHAR), '+00:00', '') AS TIMESTAMP) AS occurred_at, "
            "CAST(data AS VARCHAR) AS data FROM _src"
        )
        con.unregister("_src")

    def _build(self, con: duckdb.DuckDBPyConnection) -> None:
        con.execute("""
            CREATE TABLE task_status_events AS
            SELECT entity_id AS task_id, occurred_at,
                   json_extract_string(data, '$.from') AS from_status,
                   json_extract_string(data, '$.to') AS to_status
            FROM events WHERE event_type = 'task.status_changed'
        """)
        con.execute("""
            CREATE TABLE task_commits AS
            WITH linked AS (
                SELECT l.task_id, c.*, MAX(l.confidence) AS confidence
                FROM commit_task_links l JOIN commits c ON c.id = l.commit_id
                WHERE l.confidence >= 0.5
                GROUP BY ALL
            ), ranked AS (
                SELECT *, ROW_NUMBER() OVER (
                    PARTITION BY task_id, repository_id, COALESCE(patch_id, id) ORDER BY committed_at
                ) AS dup_rank
                FROM linked
            )
            SELECT * EXCLUDE (dup_rank) FROM ranked WHERE dup_rank = 1
        """)
        con.execute("""
            CREATE TABLE task_code AS
            SELECT tc.task_id,
                   COUNT(*) FILTER (WHERE NOT tc.is_merge) AS commits,
                   COALESCE(SUM(tc.insertions) FILTER (WHERE NOT tc.is_merge), 0) AS churn_insertions,
                   COALESCE(SUM(tc.deletions) FILTER (WHERE NOT tc.is_merge), 0) AS churn_deletions,
                   COUNT(DISTINCT tc.repository_id) AS repositories_touched,
                   COALESCE(SUM(a.functions_affected), 0) AS functions_affected,
                   COALESCE(SUM(a.types_affected), 0) AS types_affected
            FROM task_commits tc LEFT JOIN commit_analyses a ON a.commit_id = tc.id
            GROUP BY tc.task_id
        """)
        con.execute("""
            CREATE TABLE task_files AS
            SELECT tc.task_id, f.path, f.module, f.file_class, tc.repository_id,
                   COUNT(*) AS edits, SUM(f.insertions + f.deletions) AS lines
            FROM task_commits tc JOIN commit_file_changes f ON f.commit_id = tc.id
            WHERE NOT tc.is_merge
            GROUP BY ALL
        """)
        con.execute("""
            CREATE TABLE task_surface AS
            SELECT task_id, SUM(files_changed) AS final_files, SUM(insertions) AS final_insertions,
                   SUM(deletions) AS final_deletions, SUM(renames) AS final_renames,
                   SUM(binary_files) AS final_binary, SUM(functions_affected) AS final_functions,
                   SUM(types_affected) AS final_types, COUNT(*) AS final_repositories
            FROM task_change_surfaces GROUP BY task_id
        """)
        con.execute("""
            CREATE TABLE task_facts AS
            WITH top_attempts AS (
                SELECT task_id, COUNT(*) AS attempts,
                       COUNT(*) FILTER (WHERE status = 'succeeded') AS succeeded_attempts,
                       COUNT(*) FILTER (WHERE status IN ('failed', 'timed_out')) AS failed_attempts,
                       COUNT(*) FILTER (WHERE retry_of_attempt_id IS NOT NULL) AS retries,
                       SUM(duration_ms) AS execution_ms, SUM(queue_wait_ms) AS queue_wait_ms,
                       MIN(started_at) AS first_attempt_at,
                       arg_min(status, number) AS first_attempt_status
                FROM agent_attempts WHERE parent_attempt_id IS NULL GROUP BY task_id
            ), closed AS (
                SELECT task_id, MAX(occurred_at) AS closed_at
                FROM task_status_events WHERE to_status IN ('accepted', 'cancelled') GROUP BY task_id
            ), reopened AS (
                SELECT entity_id AS task_id, COUNT(*) AS reopen_count FROM events
                WHERE event_type = 'task.reopened' GROUP BY entity_id
            ), primary_project AS (
                SELECT task_id, MIN(project_id) AS project_id, COUNT(*) AS project_count FROM task_projects GROUP BY task_id
            ), repo_counts AS (
                SELECT task_id, COUNT(*) AS linked_repositories FROM task_repositories GROUP BY task_id
            )
            SELECT t.*, p.project_id, COALESCE(p.project_count, 0) AS project_count,
                   COALESCE(rc.linked_repositories, 0) AS linked_repositories,
                   COALESCE(a.attempts, 0) AS attempts, COALESCE(a.succeeded_attempts, 0) AS succeeded_attempts,
                   COALESCE(a.failed_attempts, 0) AS failed_attempts, COALESCE(a.retries, 0) AS retries,
                   a.execution_ms, a.queue_wait_ms, a.first_attempt_at, a.first_attempt_status,
                   c.closed_at, COALESCE(r.reopen_count, 0) AS reopen_count,
                   COALESCE(code.commits, 0) AS commits,
                   COALESCE(code.churn_insertions, 0) AS churn_insertions,
                   COALESCE(code.churn_deletions, 0) AS churn_deletions,
                   COALESCE(code.repositories_touched, 0) AS repositories_touched,
                   COALESCE(code.functions_affected, 0) AS churn_functions,
                   s.final_files, s.final_insertions, s.final_deletions, s.final_functions, s.final_types,
                   (SELECT COUNT(DISTINCT path) FROM task_files f WHERE f.task_id = t.id) AS files_touched,
                   (SELECT COUNT(*) FROM task_files f WHERE f.task_id = t.id AND f.edits > 1) AS repeated_file_edits,
                   CASE WHEN t.accepted_at IS NOT NULL
                        THEN epoch_ms(t.accepted_at) - epoch_ms(t.created_at) END AS elapsed_ms,
                   CASE WHEN t.accepted_at IS NOT NULL AND t.started_at IS NOT NULL
                        THEN epoch_ms(t.accepted_at) - epoch_ms(t.started_at) END AS cycle_ms
            FROM tasks t
            LEFT JOIN top_attempts a ON a.task_id = t.id
            LEFT JOIN closed c ON c.task_id = t.id
            LEFT JOIN reopened r ON r.task_id = t.id
            LEFT JOIN primary_project p ON p.task_id = t.id
            LEFT JOIN repo_counts rc ON rc.task_id = t.id
            LEFT JOIN task_code code ON code.task_id = t.id
            LEFT JOIN task_surface s ON s.task_id = t.id
        """)
        con.execute("""
            CREATE TABLE attempt_facts AS
            SELECT a.*, t.key AS task_key, t.category, t.priority,
                   TRY_CAST(json_extract(a.usage, '$.input_tokens') AS BIGINT) AS input_tokens,
                   TRY_CAST(json_extract(a.usage, '$.output_tokens') AS BIGINT) AS output_tokens,
                   (SELECT COUNT(*) FROM verification_runs v WHERE v.attempt_id = a.id) AS verification_runs,
                   (SELECT COUNT(*) FROM verification_runs v WHERE v.attempt_id = a.id AND v.status = 'passed')
                       AS verification_passed,
                   (SELECT MIN(project_id) FROM task_projects tp WHERE tp.task_id = a.task_id) AS project_id
            FROM agent_attempts a JOIN tasks t ON t.id = a.task_id
        """)

    def query(self, sql: str, params: list[Any] | None = None) -> list[dict[str, Any]]:
        with self._lock:
            con = self.connection()
            cursor = con.execute(sql, params or [])
            columns = [d[0] for d in cursor.description or []]
            return [dict(zip(columns, row, strict=True)) for row in cursor.fetchall()]

    def export(self, sql: str, params: list[Any], target: Path, fmt: str) -> Path:
        with self._lock:
            con = self.connection()
            relation = con.execute(sql, params)
            arrow = relation.to_arrow_table()
            con.register("_export", arrow)
            target.parent.mkdir(parents=True, exist_ok=True)
            options = {
                "csv": "(FORMAT CSV, HEADER)",
                "parquet": "(FORMAT PARQUET)",
                "json": "(FORMAT JSON, ARRAY TRUE)",
            }
            con.execute(f"COPY _export TO '{target.as_posix()}' {options[fmt]}")
            con.unregister("_export")
            return target

    def close(self) -> None:
        with self._lock:
            if self._con is not None:
                self._con.close()
                self._con = None
