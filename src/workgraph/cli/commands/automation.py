"""automation commands."""

from __future__ import annotations

from typing import Any

import typer

from workgraph.cli.common import (
    STATE,
    console,
    emit,
    handle_errors,
    kv,
    parse_json_arg,
    short,
    table,
    when,
    workspace_option,
    ws,
)

automation_app = typer.Typer(help="Declarative automation rules, previews and run history.", no_args_is_help=True)


@automation_app.command("list")
@handle_errors
def rule_list(workspace: str | None = workspace_option()) -> None:
    """List rules."""
    rules = STATE.app().automation.list_rules(STATE.ctx(), ws(workspace))
    emit(rules, lambda rs: table([(r.name, "on" if r.enabled else "off", r.trigger.get("type"),
                                   ",".join(r.trigger.get("event_types", [])) or r.trigger.get("cron") or "",
                                   len(r.actions), r.created_by) for r in rs],
                                 ["rule", "enabled", "trigger", "events/schedule", "actions", "author"]))


@automation_app.command("create")
@handle_errors
def rule_create(spec: str = typer.Argument(..., help="Rule JSON or @path/to/rule.json"),
                workspace: str | None = workspace_option()) -> None:
    """Create a rule from a JSON specification."""
    out = STATE.app().automation.create_rule(STATE.ctx(), ws(workspace), parse_json_arg(spec, "rule"))
    emit(out, lambda r: console.print(f"created rule {r.name}"))


@automation_app.command("update")
@handle_errors
def rule_update(name: str, spec: str, workspace: str | None = workspace_option()) -> None:
    """Replace a rule's specification."""
    out = STATE.app().automation.update_rule(STATE.ctx(), ws(workspace), name, parse_json_arg(spec, "rule"))
    emit(out, lambda r: console.print(f"updated rule {r.name} (v{r.version})"))


@automation_app.command("show")
@handle_errors
def rule_show(name: str, workspace: str | None = workspace_option()) -> None:
    """Show a rule definition."""
    emit(STATE.app().automation.get_rule(STATE.ctx(), ws(workspace), name), lambda r: kv(r.model_dump().items(), title=r.name))


@automation_app.command("enable")
@handle_errors
def rule_enable(name: str, workspace: str | None = workspace_option()) -> None:
    """Enable a rule."""
    emit(STATE.app().automation.set_enabled(STATE.ctx(), ws(workspace), name, True), lambda r: console.print(f"{r.name} enabled"))


@automation_app.command("disable")
@handle_errors
def rule_disable(name: str, workspace: str | None = workspace_option()) -> None:
    """Disable a rule."""
    emit(STATE.app().automation.set_enabled(STATE.ctx(), ws(workspace), name, False), lambda r: console.print(f"{r.name} disabled"))


@automation_app.command("trigger")
@handle_errors
def rule_trigger(name: str, task: str | None = typer.Option(None), input_json: str | None = typer.Option(None, "--input"),
                 workspace: str | None = workspace_option()) -> None:
    """Manually trigger a rule."""
    inputs: dict[str, Any] = parse_json_arg(input_json, "input") or {}
    if task:
        inputs["task"] = task
    out = STATE.app().automation.trigger(STATE.ctx(), ws(workspace), name, inputs=inputs)
    emit(out, lambda r: console.print(f"run {short(r.id)}: {r.status} ({r.reason})"))


@automation_app.command("preview")
@handle_errors
def rule_preview(rule: str | None = typer.Option(None, help="Existing rule name"),
                 spec: str | None = typer.Option(None, help="Unsaved rule JSON or @file"),
                 event: str | None = typer.Option(None, help="Event id to evaluate against"),
                 task: str | None = typer.Option(None, help="Task to use as context"),
                 workspace: str | None = workspace_option()) -> None:
    """Dry run: evaluate conditions and show planned actions without executing them."""
    out = STATE.app().automation.preview(STATE.ctx(), ws(workspace), rule=rule, spec=parse_json_arg(spec, "spec"),
                                         event_id=event, inputs={"task": task} if task else None)

    def human(r: Any) -> None:
        console.print(f"matched: {r.matched} ({r.reason})")
        table([(t["field"], t["op"], t["expected"], t["actual"], t["result"]) for t in r.condition_trace],
              ["field", "op", "expected", "actual", "result"], title="conditions")
        for action in r.action_results:
            console.print(f"  would {action['planned']}")

    emit(out, human)


@automation_app.command("runs")
@handle_errors
def rule_runs(rule: str | None = typer.Option(None), status: str | None = typer.Option(None),
              limit: int = typer.Option(50), workspace: str | None = workspace_option()) -> None:
    """Trigger and execution history."""
    runs = STATE.app().automation.runs(STATE.ctx(), ws(workspace), rule=rule, status=status, limit=limit)
    emit(runs, lambda rs: table([(short(r.id), short(r.rule_id), r.trigger_type, r.status, r.depth, r.reason[:50],
                                  when(r.created_at)) for r in rs],
                                ["run", "rule", "trigger", "status", "depth", "reason", "when"]))


@automation_app.command("notifications")
@handle_errors
def notifications(unread: bool = typer.Option(False), workspace: str | None = workspace_option()) -> None:
    """Local notifications emitted by rules."""
    rows = STATE.app().automation.notifications(STATE.ctx(), ws(workspace), unread_only=unread)
    emit(rows, lambda rs: table([(n["level"], n["title"], n["created_at"][:16]) for n in rs], ["level", "title", "when"]))


@automation_app.command("catalog")
@handle_errors
def catalog() -> None:
    """Supported triggers, actions and condition operators."""
    emit(STATE.app().automation.catalog(), lambda r: kv(r.items()))


@automation_app.command("process")
@handle_errors
def process() -> None:
    """Process pending events and due schedules now (normally done by the worker)."""
    from workgraph.core.context import RequestContext

    app = STATE.app()
    ctx = RequestContext.system("cli")
    result = {"events": app.automation.process_events(ctx), "scheduled": app.automation.run_scheduled(ctx)}
    emit(result, lambda r: kv(r.items()))
