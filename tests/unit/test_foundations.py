from __future__ import annotations

import sqlite3
import threading
from pathlib import Path

import pytest
import tomli_w
from sqlalchemy import text

from workgraph.analytics.code.classify import file_class, language_for, module_for
from workgraph.analytics.code.structure import analyze_file, available_languages, extract_symbols, parse_hunks
from workgraph.config.paths import AppPaths
from workgraph.config.settings import ConfigStore
from workgraph.core.errors import ConfigError, PermissionDenied, ValidationFailed
from workgraph.core.fsutil import require_within, safe_relative_path
from workgraph.integrations.git.cli import WorktreeEntry, normalize_remote_url, parse_worktree_porcelain
from workgraph.integrations.git.parse import parse_diff_tree, parse_meta, parse_trailers
from workgraph.persistence import migrate
from workgraph.persistence.db import Database
from workgraph.persistence.schema_version import SCHEMA_HEAD
from workgraph.security.permissions import DEFAULT_AGENT_PERMISSIONS, Permission, parse_permissions
from workgraph.security.redaction import redact, redact_text
from workgraph.verification.configs import CheckSpec

# ------------------------------------------------------------------ persistence


def test_schema_head_constant_matches_migrations():
    assert migrate.head_revision() == SCHEMA_HEAD


def test_migrations_clean_downgrade_and_existing(tmp_path):
    db = Database(tmp_path / "m.db")
    migrate.upgrade(db)
    assert migrate.is_up_to_date(db)
    with db.engine.begin() as conn:
        conn.exec_driver_sql(
            "INSERT INTO workspaces (id, slug, name, description, key_prefix, next_task_number, settings, created_at, "
            "updated_at) VALUES ('w', 's', 'n', '', 'WG', 1, '{}', '2026-01-01', '2026-01-01')"
        )
    migrate.downgrade(db, "0001")
    assert migrate.current_revision(db) == "0001"
    migrate.upgrade(db)
    with db.engine.connect() as conn:
        assert conn.exec_driver_sql("SELECT slug FROM workspaces").scalar() == "s"
    conn2 = sqlite3.connect(tmp_path / "m.db")
    assert conn2.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
    conn2.close()


def test_foreign_keys_enforced_and_rollback(app):
    with pytest.raises(Exception, match="FOREIGN KEY"), app.db.session(write=True) as session:
        session.execute(
            text(
                "INSERT INTO projects (id, workspace_id, slug, name, description, status, settings, created_at, updated_at) "
                "VALUES ('p', 'missing', 's', 'n', '', 'active', '{}', 'x', 'x')"
            )
        )
    with pytest.raises(RuntimeError), app.db.session(write=True) as session:
        session.execute(
            text(
                "INSERT INTO workspaces (id, slug, name, description, key_prefix, next_task_number, settings, created_at, "
                "updated_at) VALUES ('w2', 'rolled', 'n', '', 'WG', 1, '{}', 'x', 'x')"
            )
        )
        raise RuntimeError("abort")
    with app.db.session() as session:
        assert session.execute(text("SELECT COUNT(*) FROM workspaces WHERE slug='rolled'")).scalar() == 0


def test_concurrent_writers_serialize(app, human, ws):
    from workgraph.core.models.schemas import TaskCreate

    errors: list[Exception] = []

    def create(n: int) -> None:
        try:
            for i in range(10):
                app.tasks.create(human, ws, TaskCreate(title=f"t{n}-{i}"))
        except Exception as exc:
            errors.append(exc)

    threads = [threading.Thread(target=create, args=(n,)) for n in range(5)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors
    page = app.tasks.list(human, ws)
    assert page.total == 50
    numbers = sorted(
        int(t.key.split("-")[1])
        for t in app.tasks.list(
            human,
            ws,
            __import__("workgraph.core.models.schemas", fromlist=["TaskFilter"]).TaskFilter(limit=100),
        ).items
    )
    assert numbers == list(range(1, 51))


# ------------------------------------------------------------------ configuration


def test_config_validation_and_migration(tmp_path):
    paths = AppPaths.resolve(tmp_path)
    paths.ensure()
    store = ConfigStore(paths)
    paths.config_file.write_text(tomli_w.dumps({"schema_version": 1, "agents": {"enabled": True}}))
    assert store.load_global().execution.enabled is True
    assert store.migrate_files() == [paths.config_file]
    assert "schema_version = 2" in paths.config_file.read_text()
    paths.config_file.write_text(tomli_w.dumps({"server": {"port": 0}}))
    with pytest.raises(ConfigError):
        store.load_global()
    assert store.validate_files()
    paths.config_file.write_text("not = [valid")
    with pytest.raises(ConfigError):
        store.load_global()


def test_workspace_overrides(tmp_path):
    paths = AppPaths.resolve(tmp_path)
    paths.ensure()
    store = ConfigStore(paths)
    store.workspace_file("team").write_text(tomli_w.dumps({"execution": {"max_concurrent_runs": 7}}))
    assert store.effective("team").execution.max_concurrent_runs == 7
    assert store.effective(None).execution.max_concurrent_runs == 2


# ------------------------------------------------------------------ security


def test_permissions():
    assert Permission.RUN_START not in DEFAULT_AGENT_PERMISSIONS
    assert Permission.TASK_ACCEPT not in DEFAULT_AGENT_PERMISSIONS
    assert parse_permissions(["run:start"]) == {Permission.RUN_START}
    with pytest.raises(PermissionDenied):
        parse_permissions(["root:everything"])


def test_redaction():
    payload = {
        "api_key": "abc",
        "nested": {"cmd": "curl -H 'Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123'"},
        "list": ["ghp_" + "a" * 30],
        "ok": "hello",
    }
    out = redact(payload)
    assert out["api_key"] == "[REDACTED]"
    assert "abcdefghijklmnop" not in out["nested"]["cmd"]
    assert out["list"][0] == "[REDACTED]" and out["ok"] == "hello"
    assert redact_text("password=hunter2 rest") == "[REDACTED] rest"
    assert len(redact_text("x" * 100, 10)) < 60


def test_path_boundaries(tmp_path):
    with pytest.raises(ValidationFailed):
        safe_relative_path("../etc/passwd")
    with pytest.raises(ValidationFailed):
        safe_relative_path("/abs")
    assert safe_relative_path("a/b.txt") == Path("a/b.txt")
    with pytest.raises(ValidationFailed):
        require_within(tmp_path / ".." / "x", tmp_path)


def test_check_spec_rejects_shell_unless_opted_in():
    with pytest.raises(ValueError):
        CheckSpec(name="x", command=["bash", "-c", "rm -rf /"])
    assert CheckSpec(name="x", command=["bash", "-c", "true"], allow_shell=True).allow_shell
    with pytest.raises(ValueError):
        CheckSpec(name="x", command=["pytest"], env={"bad name": "1"})


# ------------------------------------------------------------------ git parsing


def test_parse_diff_tree_and_meta():
    sha = "a" * 40
    out = (
        f"{sha}\0:100644 100644 1111111 2222222 R087\0old.py\0new.py\0:000000 100644 0000000 3333333 A\0bin.dat\0"
        "3\t1\t\0old.py\0new.py\0-\t-\tbin.dat\0"
    )
    diffs = parse_diff_tree(out)
    files = {f.path: f for f in diffs[sha].files}
    assert files["new.py"].old_path == "old.py" and files["new.py"].similarity == 87
    assert files["new.py"].insertions == 3 and files["new.py"].deletions == 1
    assert files["bin.dat"].binary and files["bin.dat"].status == "A"
    meta = parse_meta(
        f"\x1e{sha}\x1ftree\x1fp1 p2\x1fA\x1fa@x\x1f2026-01-01T00:00:00+00:00\x1fC\x1fc@x\x1f"
        "2026-01-01T00:00:00+00:00\x1fTask-ID: WG-1\x1dReviewed-by: X\x1fSubject\n\nBody\n"
    )
    assert meta[0].parents == ["p1", "p2"] and meta[0].subject == "Subject"
    assert meta[0].trailers == [{"key": "Task-ID", "value": "WG-1"}, {"key": "Reviewed-by", "value": "X"}]
    assert parse_trailers("") == []


def test_normalize_remote_url():
    variants = [
        "git@github.com:Org/Repo.git",
        "https://github.com/Org/Repo",
        "ssh://git@GitHub.com/Org/Repo.git",
        "https://user:pw@github.com/Org/Repo.git/",
    ]
    assert {normalize_remote_url(v) for v in variants} == {"github.com/Org/Repo"}


def test_parse_worktree_porcelain_entries_in_both_framings():
    z_output = (
        "worktree C:/dev/repo\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0"
        "worktree C:/dev/wt-detached\0HEAD 2222222222222222222222222222222222222222\0detached\0\0"
        "worktree C:/dev/wt-locked\0HEAD 3333333333333333333333333333333333333333\0"
        "branch refs/heads/feature/x\0locked rebase in progress\0\0"
        "worktree C:/dev/wt-gone\0HEAD 4444444444444444444444444444444444444444\0"
        "branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0"
        "worktree C:/dev/bare.git\0bare\0\0"
    )
    entries = parse_worktree_porcelain(z_output, nul=True)
    assert entries == [
        WorktreeEntry(Path("C:/dev/repo"), "1" * 40, "main"),
        WorktreeEntry(Path("C:/dev/wt-detached"), "2" * 40, None, detached=True),
        WorktreeEntry(Path("C:/dev/wt-locked"), "3" * 40, "feature/x", locked=True),
        WorktreeEntry(Path("C:/dev/wt-gone"), "4" * 40, "old", prunable=True),
        WorktreeEntry(Path("C:/dev/bare.git"), bare=True),
    ]
    newline_output = z_output.replace("\0", "\n")
    assert parse_worktree_porcelain(newline_output, nul=False) == entries
    assert parse_worktree_porcelain("", nul=True) == []


# ------------------------------------------------------------------ structural analysis

SOURCES = {
    "python": (b"class A:\n    def m(self):\n        return 1\n", "A.m"),
    "typescript": (b"export class Svc {\n  run(): number {\n    return 1;\n  }\n}\n", "Svc.run"),
    "tsx": (b"function App() {\n  return <div/>;\n}\n", "App"),
    "javascript": (b"function go() {\n  return 1;\n}\n", "go"),
    "go": (b"package main\n\nfunc Run() int {\n\treturn 1\n}\n", "Run"),
    "rust": (b"struct S;\nimpl S {\n    fn run(&self) -> i32 {\n        1\n    }\n}\n", "run"),
    "c_sharp": (b"class Svc {\n  int Run() {\n    return 1;\n  }\n}\n", "Svc.Run"),
}


@pytest.mark.parametrize("language", sorted(SOURCES))
def test_symbol_extraction_all_languages(language):
    source, expected = SOURCES[language]
    symbols = extract_symbols(source, language)
    assert symbols is not None
    assert any(s.name.endswith(expected) for s in symbols), [s.name for s in symbols]


def test_analyze_file_changes_and_fallback():
    old = b"def a():\n    return 1\n\ndef b():\n    return 2\n"
    new = b"def a():\n    return 10\n\ndef c():\n    return 3\n"
    deleted, added = parse_hunks("@@ -2 +2 @@\n-    return 1\n+    return 10\n@@ -4,2 +4,2 @@\n")
    result = analyze_file("m.py", "python", old, new, deleted, added)
    changes = {(c.name, c.change) for c in result.changes}
    assert ("a", "modified") in changes and ("b", "deleted") in changes and ("c", "added") in changes
    assert analyze_file("x.txt", None, b"", b"", [], [1]).mode == "fallback"
    assert set(available_languages()) >= {"python", "typescript", "javascript", "go", "rust", "c_sharp"}


def test_file_classification():
    assert file_class("tests/test_api.py") == "test"
    assert file_class("src/app.test.ts") == "test"
    assert file_class("docs/guide.md") == "docs"
    assert file_class("pyproject.toml") == "config"
    assert file_class("src/workgraph/app.py") == "production"
    assert language_for("a.rs") == "rust" and language_for("a.unknown") is None
    assert module_for("src/workgraph/core/x.py") == "src/workgraph/core"
    assert module_for("README.md") == "/"
