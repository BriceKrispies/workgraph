from __future__ import annotations

from datetime import timedelta

import pytest

from tests.helpers import drain
from workgraph.automation.cron import Cron
from workgraph.automation.rules import Condition, evaluate
from workgraph.core.clock import utcnow
from workgraph.core.errors import PermissionDenied, ValidationFailed
from workgraph.core.models.schemas import TaskCreate


def test_condition_evaluator_trace():
    cond = Condition.model_validate({"all": [
        {"field": "task.priority", "op": "in", "value": ["high", "critical"]},
        {"not": {"field": "task.tags", "op": "contains", "value": "skip"}},
        {"any": [{"field": "task.title", "op": "matches", "value": "(?i)crash"},
                 {"field": "classification.category", "op": "eq", "value": "bug_fix"}]},
    ]})
    trace: list = []
    ctx = {"task": {"priority": "high", "tags": [], "title": "App crash on start"}}
    assert evaluate(cond, ctx, trace)
    assert [t["result"] for t in trace] == [True, False, True]
    assert not evaluate(cond, {"task": {"priority": "low", "tags": [], "title": "x"}})
    assert evaluate(Condition.model_validate({"field": "missing.path", "op": "not_exists"}), {})


def test_cron():
    cron = Cron("*/15 9-17 * * 1-5")
    monday = utcnow().replace(year=2026, month=10, day=19, hour=8, minute=59, second=0, microsecond=0)
    assert cron.next_after(monday).hour == 9 and cron.next_after(monday).minute == 0
    with pytest.raises(ValidationFailed):
        Cron("61 * * * *")


def rule(**overrides):
    base = {
        "name": "tag bugs",
        "trigger": {"type": "event", "event_types": ["task.created"]},
        "conditions": {"field": "task.title", "op": "matches", "value": "(?i)bug|crash"},
        "actions": [{"type": "add_tags", "tags": ["needs-triage"]},
                    {"type": "notify", "title": "New bug {{task.key}}: {{task.title}}"}],
    }
    base.update(overrides)
    return base


def test_event_rule_matches_and_is_idempotent(app, human, ws):
    created = app.automation.create_rule(human, ws, rule())
    bug = app.tasks.create(human, ws, TaskCreate(title="Crash on save"))
    plain = app.tasks.create(human, ws, TaskCreate(title="Add button"))
    drain(app)
    assert "needs-triage" in app.tasks.get(human, ws, bug.key).tags
    assert "needs-triage" not in app.tasks.get(human, ws, plain.key).tags
    runs = app.automation.runs(human, ws, rule=created.name)
    statuses = sorted(r.status for r in runs)
    assert statuses == ["not_matched", "succeeded"]
    matched = next(r for r in runs if r.status == "succeeded")
    assert matched.condition_trace[0]["result"] is True
    assert [a["status"] for a in matched.action_results] == ["succeeded", "succeeded"]
    notes = app.automation.notifications(human, ws)
    assert notes[0]["title"] == f"New bug {bug.key}: Crash on save"
    # re-processing the event log creates no duplicate runs
    from workgraph.core.context import RequestContext

    with app.db.session(write=True) as session:
        from workgraph.persistence.orm import SystemCursor

        session.get(SystemCursor, "automation").value = 0
    app.automation.process_events(RequestContext.system("test"))
    assert len(app.automation.runs(human, ws, rule=created.name)) == 2


def test_recursion_guard(app, human, ws):
    # Each run creates a child task, which emits task.created again -> bounded by max_depth.
    app.automation.create_rule(human, ws, {
        "name": "fractal",
        "trigger": {"type": "event", "event_types": ["task.created"]},
        "actions": [{"type": "create_child_tasks", "children": [{"title": "child of {{task.key}}"}]}],
        "max_depth": 2,
    })
    app.tasks.create(human, ws, TaskCreate(title="root"))
    drain(app)
    tasks = app.tasks.list(human, ws).items
    assert len(tasks) == 3  # root + depth1 child + depth2 child
    runs = app.automation.runs(human, ws, rule="fractal")
    skipped = [r for r in runs if r.status == "skipped"]
    assert skipped and "recursion guard" in skipped[0].reason


def test_dry_run_preview(app, human, ws):
    task = app.tasks.create(human, ws, TaskCreate(title="Crash when idle"))
    preview = app.automation.preview(human, ws, spec=rule(name="unsaved"), inputs={"task": task.key})
    assert preview.dry_run and preview.status == "previewed"
    assert preview.action_results[0]["planned"].startswith("add tags")
    assert "needs-triage" not in app.tasks.get(human, ws, task.key).tags


def test_manual_trigger_and_agent_restrictions(app, human, agent_ctx, ws):
    task = app.tasks.create(human, ws, TaskCreate(title="Something"))
    app.automation.create_rule(human, ws, {
        "name": "escalate", "trigger": {"type": "manual"},
        "actions": [{"type": "update_task", "priority": "critical"}],
    })
    run = app.automation.trigger(agent_ctx, ws, "escalate", inputs={"task": task.key})
    assert run.status == "queued"
    drain(app)
    assert app.tasks.get(human, ws, task.key).priority == "critical"
    with pytest.raises(PermissionDenied):
        app.automation.create_rule(agent_ctx, ws, rule(name="agent rule"))
    app.automation.create_rule(human, ws, rule(name="event only"))
    with pytest.raises(PermissionDenied):
        app.automation.trigger(agent_ctx, ws, "event only")


def test_scheduled_rule(app, human, ws):
    created = app.automation.create_rule(human, ws, {
        "name": "weekly report", "trigger": {"type": "schedule", "interval_seconds": 3600},
        "actions": [{"type": "notify", "title": "tick {{input.scheduled_for}}"}],
    })
    from workgraph.core.context import RequestContext

    ctx = RequestContext.system("test")
    assert app.automation.run_scheduled(ctx) == 0
    later = (created.next_run_at or utcnow()) + timedelta(seconds=1)
    assert app.automation.run_scheduled(ctx, now=later) == 1
    assert app.automation.run_scheduled(ctx, now=later) == 0
    drain(app)
    assert app.automation.notifications(human, ws)[0]["title"].startswith("tick ")


def test_failing_action_retries_then_dead_letters(app, human, ws):
    app.automation.create_rule(human, ws, {
        "name": "bad dep", "trigger": {"type": "event", "event_types": ["task.created"]},
        "actions": [{"type": "create_dependency", "depends_on": "WG-999"}], "retry_max_attempts": 2,
    })
    app.tasks.create(human, ws, TaskCreate(title="x"))
    drain(app)
    run = app.automation.runs(human, ws, rule="bad dep")[0]
    assert run.status == "failed" and "WG-999" in (run.error or "")
    jobs, _ = app.queue.list(kind="automation_action")
    assert jobs[0].status == "queued" and jobs[0].attempts == 1  # retry scheduled with backoff
    with app.db.session(write=True) as session:
        from workgraph.persistence.orm import Job

        session.get(Job, jobs[0].id).run_at = utcnow()
    drain(app)
    assert app.queue.get(jobs[0].id).status == "dead"


def test_invalid_rules_rejected(app, human, ws):
    with pytest.raises(ValidationFailed):
        app.automation.create_rule(human, ws, rule(trigger={"type": "event", "event_types": ["not.real"]}))
    with pytest.raises(ValidationFailed):
        app.automation.create_rule(human, ws, rule(actions=[{"type": "run_shell", "command": "rm -rf /"}]))
