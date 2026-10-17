from __future__ import annotations

import io
import json
import subprocess
import sys
from pathlib import Path

from sqlalchemy import select

from workgraph.core.models.enums import AttemptStatus
from workgraph.core.models.schemas import TaskCreate
from workgraph.core.services import resolve
from workgraph.core.services.attempts import new_attempt
from workgraph.integrations.claude_code import installer
from workgraph.integrations.claude_code.hook_cli import run_hook
from workgraph.persistence.orm import AgentAttempt, AgentInternalTask, AgentSession, HookEvent, SubagentRun, ToolEvent


def hook(app, payload, env=None, monkeypatch=None, received_at="2026-10-17T10:00:00+00:00"):
    record = {"received_at": received_at, "env": env or {}, "payload": payload}
    return app.hooks.ingest(record)


def base(event, session="sess-1", **kw):
    return {"session_id": session, "transcript_path": "/t.jsonl", "cwd": "/work", "hook_event_name": event, **kw}


def make_attempt(app, human, ws):
    task = app.tasks.create(human, ws, TaskCreate(title="Hooked", status="ready"))
    with app.db.session(write=True) as session:
        t = resolve.task(session, None, task.id)
        attempt = new_attempt(session, human, t, provider="claude_code", adapter="claude_code", status=AttemptStatus.RUNNING)
        return task, attempt.id


def test_session_correlation_tools_subagents_and_internal_tasks(app, human, ws):
    task, attempt_id = make_attempt(app, human, ws)
    env = {"attempt_id": attempt_id, "task_id": task.id}
    assert hook(app, base("SessionStart", source="startup", model="claude-x"), env).status == "ingested"
    hook(app, base("PreToolUse", tool_name="Bash", tool_use_id="t1",
                   tool_input={"command": "export TOKEN=sk-ant-abcdefghijklmnopqrstuvwxyz && pytest"}), env,
         received_at="2026-10-17T10:00:01+00:00")
    hook(app, base("PostToolUse", tool_name="Bash", tool_use_id="t1", tool_input={"command": "pytest"},
                   tool_response={"stdout": "ok"}, duration_ms=1200), env, received_at="2026-10-17T10:00:02+00:00")
    hook(app, base("PostToolUseFailure", tool_name="Edit", tool_use_id="t2", tool_input={"file_path": "/work/a.py"},
                   error="Exit code 1 password=hunter2", duration_ms=5), env, received_at="2026-10-17T10:00:03+00:00")
    hook(app, base("SubagentStart", agent_id="agent-1", agent_type="Explore"), env, received_at="2026-10-17T10:00:04+00:00")
    hook(app, base("SubagentStop", agent_id="agent-1", agent_type="Explore", last_assistant_message="Found 3 issues",
                   stop_hook_active=False), env, received_at="2026-10-17T10:00:05+00:00")
    hook(app, base("TaskCreated", task_id="task-001", task_subject=f"Implement part of {task.key}"), env,
         received_at="2026-10-17T10:00:06+00:00")
    hook(app, base("TaskCompleted", task_id="task-001", task_subject=f"Implement part of {task.key}"), env,
         received_at="2026-10-17T10:00:07+00:00")
    hook(app, base("Stop", last_assistant_message="done", stop_hook_active=False), env, received_at="2026-10-17T10:00:08+00:00")
    hook(app, base("SessionEnd", reason="other"), env, received_at="2026-10-17T10:00:09+00:00")
    with app.db.session() as session:
        agent_session = session.scalar(select(AgentSession).where(AgentSession.external_session_id == "sess-1"))
        assert agent_session.binding_source == "workgraph_launched"
        assert agent_session.attempt_id == attempt_id and agent_session.task_id == task.id
        assert agent_session.tool_calls == 2 and agent_session.tool_failures == 1
        assert agent_session.end_reason == "other"
        tools = session.scalars(select(ToolEvent).order_by(ToolEvent.occurred_at)).all()
        assert [t.phase for t in tools] == ["pre", "success", "failure"]
        assert tools[2].summary["file_path"] == "/work/a.py"
        assert "hunter2" not in (tools[2].error_excerpt or "")
        pre = session.scalar(select(HookEvent).where(HookEvent.hook_event_name == "PreToolUse"))
        assert "sk-ant" not in json.dumps(pre.payload)  # tool payloads are summarised, not stored
        sub = session.scalar(select(SubagentRun))
        assert sub.agent_type == "Explore" and sub.stopped_at is not None
        child = session.get(AgentAttempt, sub.attempt_id)
        assert child.parent_attempt_id == attempt_id and child.status == "succeeded"
        internal = session.scalar(select(AgentInternalTask))
        assert internal.status == "completed" and internal.task_id == task.id
        parent = session.get(AgentAttempt, attempt_id)
        assert parent.telemetry["hook_tool_calls"] == 2


def test_idempotent_ingestion(app, human, ws):
    payload = base("SessionStart", session="dup", source="startup")
    assert hook(app, payload).status == "ingested"
    assert hook(app, payload).status == "duplicate"
    with app.db.session() as session:
        assert len(session.scalars(select(HookEvent)).all()) == 1


def test_unbound_then_explicit_binding_by_cwd(app, human, ws, tmp_path):
    task = app.tasks.create(human, ws, TaskCreate(title="External session"))
    app.hooks.bind(human, ws, task.key, cwd=str(tmp_path))
    payload = base("SessionStart", session="ext-1", source="startup")
    payload["cwd"] = str(tmp_path / "sub")
    hook(app, payload)
    sessions = app.hooks.sessions()
    assert sessions[0]["binding"] == "explicit_binding" and sessions[0]["task_id"] == task.id
    unbound = base("SessionStart", session="ext-2")
    unbound["cwd"] = "/elsewhere"
    hook(app, unbound)
    assert next(s for s in app.hooks.sessions() if s["external_session_id"] == "ext-2")["binding"] == "unbound"


def test_spooling_and_replay(app, home, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    monkeypatch.setattr(sys, "stdin", io.TextIOWrapper(io.BytesIO(json.dumps(base("SessionStart", session="sp")).encode())))
    # Simulate an un-ingestable moment: the schema check fails because the database is "locked/missing".
    import workgraph.integrations.claude_code.ingest as ingest_mod

    def broken(*_a, **_k):
        raise RuntimeError("database is locked")

    monkeypatch.setattr(ingest_mod, "ingest_direct", broken)
    assert run_hook([]) == 0
    spooled = list(app.paths.spool_dir.glob("*.json"))
    assert len(spooled) == 1
    record = json.loads(spooled[0].read_text())
    assert record["spool_reason"].startswith("RuntimeError")
    counts = app.hooks.replay_spool()
    assert counts["ingested"] == 1
    assert not list(app.paths.spool_dir.glob("*.json"))
    assert app.hooks.replay_spool() == {"ingested": 0, "duplicate": 0, "failed": 0}


def test_hook_command_end_to_end_subprocess(app, home):
    """The real CLI entrypoint ingests directly and exits 0 with no stdout."""
    payload = base("SessionStart", session="proc-1", source="startup")
    env = {**__import__("os").environ, "WORKGRAPH_HOME": str(home)}
    proc = subprocess.run([sys.executable, "-m", "workgraph", "claude", "hook"], input=json.dumps(payload),
                          capture_output=True, text=True, env=env, timeout=60)
    assert proc.returncode == 0 and proc.stdout == ""
    assert any(s["external_session_id"] == "proc-1" for s in app.hooks.sessions())
    bad = subprocess.run([sys.executable, "-m", "workgraph", "claude", "hook"], input="{not json",
                         capture_output=True, text=True, env=env, timeout=60)
    assert bad.returncode == 0


def test_installer_merges_and_uninstalls_only_its_entries(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "claude"))
    settings = tmp_path / "claude" / "settings.json"
    settings.parent.mkdir(parents=True)
    existing = {
        "model": "opus",
        "permissions": {"allow": ["Bash(git *)"]},
        "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "my-linter"}]}]},
    }
    settings.write_text(json.dumps(existing))
    exe = str(tmp_path / "bin" / "workgraph.exe")
    result = installer.install("user", command=exe)
    assert set(result["added"]) == set(installer.DEFAULT_EVENTS)
    assert result["backup"] and Path(result["backup"]).exists()
    data = json.loads(settings.read_text())
    assert data["model"] == "opus" and data["permissions"] == existing["permissions"]
    pre = data["hooks"]["PreToolUse"]
    assert pre[0]["hooks"][0]["command"] == "my-linter"
    ours = pre[1]["hooks"][0]
    assert ours["args"] == ["claude", "hook"] and ours["async"] is True
    assert installer.install("user", command=exe)["added"] == []  # idempotent
    status = installer.status("user")
    assert status["missing_events"] == []
    removed = installer.uninstall("user")
    assert set(removed["removed"]) == set(installer.DEFAULT_EVENTS)
    after = json.loads(settings.read_text())
    assert after == existing


def test_installer_refuses_invalid_json(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    (tmp_path / "settings.json").write_text("{ broken")
    import pytest

    from workgraph.core.errors import ConfigError

    with pytest.raises(ConfigError):
        installer.install("user", command="workgraph")
    assert (tmp_path / "settings.json").read_text() == "{ broken"
