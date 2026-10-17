"""Backups, restores and workspace export/import.

* Backups use SQLite's online backup API (consistent even while Workgraph runs) and
  are written as a zip containing the database, configuration files and a manifest.
* Restores verify integrity first, keep the current database as a safety copy,
  replace files atomically and then migrate the restored schema forward.
* Workspace export/import moves one workspace's records as JSON Lines in foreign-key
  order; nothing is deleted on import (it refuses to overwrite an existing workspace).
"""

from __future__ import annotations

import json
import shutil
import sqlite3
import tempfile
import zipfile
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from sqlalchemy import Table, select, text

from workgraph import __version__
from workgraph.config.paths import AppPaths
from workgraph.core.errors import ConflictError, ValidationFailed
from workgraph.persistence.orm import Base
from workgraph.persistence.types import UTCDateTime

SKIP_EXPORT = {
    "system_cursors",
    "workers",
    "resource_leases",
    "job_resource_claims",
    "job_runs",
    "hook_events",
    "tool_events",
    "alembic_version",
}


def _sqlite_backup(source: Path, target: Path) -> None:
    src = sqlite3.connect(source, timeout=30)
    dst = sqlite3.connect(target)
    try:
        src.backup(dst)
    finally:
        dst.close()
        src.close()


def _integrity(path: Path) -> str:
    conn = sqlite3.connect(path)
    try:
        return str(conn.execute("PRAGMA integrity_check").fetchone()[0])
    finally:
        conn.close()


def create_backup(
    paths: AppPaths, target: Path | None = None, *, include_artifacts: bool = False
) -> dict[str, Any]:
    stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    target = target or paths.backups_dir / f"workgraph-backup-{stamp}.zip"
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        db_copy = Path(tmp) / "workgraph.db"
        _sqlite_backup(paths.database_file, db_copy)
        conn = sqlite3.connect(db_copy)
        try:
            revision = conn.execute("SELECT version_num FROM alembic_version").fetchone()
        finally:
            conn.close()
        manifest = {
            "format": "workgraph-backup-v1",
            "created_at": stamp,
            "workgraph_version": __version__,
            "schema_revision": revision[0] if revision else None,
            "includes_artifacts": include_artifacts,
        }
        tmp_zip = target.with_suffix(".partial")
        with zipfile.ZipFile(tmp_zip, "w", compression=zipfile.ZIP_DEFLATED) as zf:
            zf.write(db_copy, "workgraph.db")
            if paths.config_file.exists():
                zf.write(paths.config_file, "config/config.toml")
            if paths.workspace_config_dir.exists():
                for file in sorted(paths.workspace_config_dir.glob("*.toml")):
                    zf.write(file, f"config/workspaces/{file.name}")
            if include_artifacts and paths.artifacts_dir.exists():
                for file in paths.artifacts_dir.rglob("*"):
                    if file.is_file():
                        zf.write(file, f"artifacts/{file.relative_to(paths.artifacts_dir).as_posix()}")
            zf.writestr("manifest.json", json.dumps(manifest, indent=2))
        tmp_zip.replace(target)
    return {"path": str(target), "bytes": target.stat().st_size, **manifest}


def restore_backup(
    paths: AppPaths, archive: Path, *, with_config: bool = False, force: bool = False
) -> dict[str, Any]:
    if not archive.exists():
        raise ValidationFailed(f"{archive} does not exist")
    with tempfile.TemporaryDirectory() as tmp:
        with zipfile.ZipFile(archive) as zf:
            names = set(zf.namelist())
            if "manifest.json" not in names or "workgraph.db" not in names:
                raise ValidationFailed(f"{archive} is not a Workgraph backup")
            manifest = json.loads(zf.read("manifest.json"))
            for name in names:
                target = (Path(tmp) / name).resolve()
                if Path(tmp).resolve() not in target.parents:
                    raise ValidationFailed(f"unsafe path in archive: {name}")
            zf.extractall(tmp)
        restored_db = Path(tmp) / "workgraph.db"
        integrity = _integrity(restored_db)
        if integrity != "ok":
            raise ValidationFailed(f"backup database failed integrity check: {integrity}")
        if paths.database_file.exists() and not force:
            conn = sqlite3.connect(paths.database_file)
            try:
                live = conn.execute(
                    "SELECT COUNT(*) FROM workers WHERE status = 'running' "
                    "AND heartbeat_at > strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now', '-60 seconds')"
                ).fetchone()[0]
            except sqlite3.Error:
                live = 0
            finally:
                conn.close()
            if live:
                raise ConflictError(
                    "a worker is running; stop Workgraph processes before restoring (or pass --force)"
                )
        paths.ensure()
        safety = None
        if paths.database_file.exists():
            stamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
            safety = paths.backups_dir / f"pre-restore-{stamp}.db"
            _sqlite_backup(paths.database_file, safety)
        for suffix in ("-wal", "-shm"):
            sidecar = paths.database_file.with_name(paths.database_file.name + suffix)
            if sidecar.exists():
                sidecar.unlink()
        staged = paths.database_file.with_suffix(".restoring")
        shutil.copy2(restored_db, staged)
        staged.replace(paths.database_file)
        if with_config:
            config_dir = Path(tmp) / "config"
            if (config_dir / "config.toml").exists():
                shutil.copy2(config_dir / "config.toml", paths.config_file)
            for file in (
                (config_dir / "workspaces").glob("*.toml") if (config_dir / "workspaces").exists() else []
            ):
                shutil.copy2(file, paths.workspace_config_dir / file.name)
        artifacts = Path(tmp) / "artifacts"
        if artifacts.exists():
            shutil.copytree(artifacts, paths.artifacts_dir, dirs_exist_ok=True)
    from workgraph.persistence import migrate
    from workgraph.persistence.db import Database

    db = Database(paths.database_file)
    try:
        before = migrate.current_revision(db)
        migrate.upgrade(db)
        after = migrate.current_revision(db)
    finally:
        db.dispose()
    return {
        "restored_from": str(archive),
        "manifest": manifest,
        "safety_copy": str(safety) if safety else None,
        "schema_before": before,
        "schema_after": after,
    }


# ---------------------------------------------------------------------------- workspace export / import


def _row_dict(row: Any) -> dict[str, Any]:
    out = {}
    for key, value in row._mapping.items():
        if isinstance(value, bytes):
            out[key] = {"__bytes__": value.hex()}
        elif isinstance(value, datetime):
            out[key] = value.isoformat()
        else:
            out[key] = value
    return out


def export_workspace(db: Any, workspace_id: str, target: Path) -> dict[str, Any]:
    """Write one workspace's rows as JSON Lines (one ``{"table":..., "row":...}`` per line)."""
    exported_ids: dict[str, set[str]] = {}
    counts: dict[str, int] = {}
    target.parent.mkdir(parents=True, exist_ok=True)
    with db.engine.connect() as conn, target.open("w", encoding="utf-8") as out:
        out.write(
            json.dumps(
                {
                    "format": "workgraph-workspace-export-v1",
                    "workspace_id": workspace_id,
                    "workgraph_version": __version__,
                }
            )
            + "\n"
        )
        for table in Base.metadata.sorted_tables:
            if table.name in SKIP_EXPORT:
                continue
            stmt = _scoped_select(table, workspace_id, exported_ids)
            if stmt is None:
                continue
            rows = conn.execute(stmt).fetchall()
            pk = list(table.primary_key.columns)
            ids: set[str] = set()
            for row in rows:
                data = _row_dict(row)
                out.write(json.dumps({"table": table.name, "row": data}, default=str) + "\n")
                if len(pk) == 1:
                    ids.add(str(data[pk[0].name]))
            exported_ids[table.name] = ids
            counts[table.name] = len(rows)
    return {"path": str(target), "tables": {k: v for k, v in counts.items() if v}}


def _scoped_select(table: Table, workspace_id: str, exported: dict[str, set[str]]) -> Any:
    cols = table.columns
    if table.name == "workspaces":
        return select(table).where(cols["id"] == workspace_id)
    if "workspace_id" in cols:
        return select(table).where(cols["workspace_id"] == workspace_id)
    for fk in table.foreign_keys:
        parent = fk.column.table.name
        if parent in exported:
            return select(table).where(fk.parent.in_(list(exported[parent]) or [""]))
    return None


def import_workspace(db: Any, source: Path) -> dict[str, Any]:
    lines = source.read_text(encoding="utf-8").splitlines()
    header = json.loads(lines[0])
    if header.get("format") != "workgraph-workspace-export-v1":
        raise ValidationFailed("not a Workgraph workspace export")
    tables = {t.name: t for t in Base.metadata.sorted_tables}
    counts: dict[str, int] = {}
    with db.engine.begin() as conn:
        # Self-referencing rows (parent tasks, nested attempts) may appear in any order.
        conn.exec_driver_sql("PRAGMA defer_foreign_keys = ON")
        exists = conn.execute(
            text("SELECT 1 FROM workspaces WHERE id = :id"), {"id": header["workspace_id"]}
        ).first()
        if exists:
            raise ConflictError("this workspace already exists in the target database; nothing was imported")
        for line in lines[1:]:
            item = json.loads(line)
            table = tables[item["table"]]
            row = {}
            for key, value in item["row"].items():
                if isinstance(value, dict) and "__bytes__" in value:
                    value = bytes.fromhex(value["__bytes__"])
                elif isinstance(value, str) and isinstance(table.columns[key].type, UTCDateTime):
                    value = datetime.fromisoformat(value)
                row[key] = value
            conn.execute(table.insert().values(**row))
            counts[table.name] = counts.get(table.name, 0) + 1
    return {"workspace_id": header["workspace_id"], "tables": counts}
