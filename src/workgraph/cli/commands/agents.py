"""attempt, agent, run and verify commands."""

from __future__ import annotations

import time
from typing import Any

import typer

from workgraph.cli.common import (
    STATE,
    console,
    emit,
    fail,
    handle_errors,
    kv,
    parse_json_arg,
    short,
    table,
    when,
    workspace_option,
    ws,
)
from workgraph.core.models.enums import TERMINAL_ATTEMPT_STATUSES, AttemptStatus
from workgraph.core.models.schemas import AttemptCreate

attempt_app = typer.Typer(help="Inspect and record agent attempts.", no_args_is_help=True)
agent_app = typer.Typer(help="Configured agent adapters and their capabilities.", no_args_is_help=True)
run_app = typer.Typer(help="Start, watch, cancel, retry and rerun agent runs.", no_args_is_help=True)
verify_app = typer.Typer(help="Verification checks and acceptance evidence.", no_args_is_help=True)


def _fmt_ms(ms: int | None) -> str:
    return "" if ms is None else f"{ms / 1000:.1f}s"


def _attempt_rows(items: list[Any]) -> list[tuple[Any, ...]]:
    return [
        (
            short(a.id),
            a.task_id[:8],
            a.number,
            a.status,
            a.provider,
            a.model or "",
            _fmt_ms(a.duration_ms),
            _fmt_ms(a.queue_wait_ms),
            a.exit_reason or "",
        )
        for a in items
    ]


ATTEMPT_HEAD = ["attempt", "task", "#", "status", "provider", "model", "duration", "queued", "exit"]


@attempt_app.command("list")
@handle_errors
def attempt_list(
    task: str | None = typer.Option(None),
    status: list[str] = typer.Option(None, "--status"),
    provider: str | None = typer.Option(None),
    limit: int = typer.Option(50),
    workspace: str | None = workspace_option(),
) -> None:
    """List attempts."""
    page = STATE.app().attempts.list(
        STATE.ctx(), ws(workspace), task=task, statuses=status or None, provider=provider, limit=limit
    )
    emit(page, lambda p: table(_attempt_rows(p.items), ATTEMPT_HEAD))


@attempt_app.command("show")
@handle_errors
def attempt_show(attempt_id: str) -> None:
    """Show an attempt with telemetry and provenance."""
    a = STATE.app().attempts.get(STATE.ctx(), attempt_id)
    emit(
        a,
        lambda x: kv(
            [
                ("id", x.id),
                ("task", x.task_id),
                ("number", x.number),
                ("status", x.status),
                ("provider", x.provider),
                ("adapter", x.adapter),
                ("model", x.model),
                ("session", x.session_id),
                ("parent", x.parent_attempt_id),
                ("retry of", x.retry_of_attempt_id),
                ("rerun of", x.rerun_of_attempt_id),
                ("queued", when(x.queued_at)),
                ("started", when(x.started_at)),
                ("finished", when(x.finished_at)),
                ("duration", _fmt_ms(x.duration_ms)),
                ("queue wait", _fmt_ms(x.queue_wait_ms)),
                ("exit", x.exit_reason),
                ("error", x.error),
                ("usage", x.usage or "not measured"),
                ("usage source", x.usage_source),
                ("cost", x.cost_usd if x.cost_usd is not None else "not measured"),
                ("modified repos", x.modified_repository_ids),
                ("telemetry", x.telemetry),
            ],
            title="attempt",
        ),
    )


@attempt_app.command("create")
@handle_errors
def attempt_create(
    task: str,
    provider: str = typer.Option("external"),
    model: str | None = typer.Option(None),
    session_id: str | None = typer.Option(None),
    workspace: str | None = workspace_option(),
) -> None:
    """Record an attempt by an externally-run agent."""
    out = STATE.app().attempts.create(
        STATE.ctx(),
        ws(workspace),
        AttemptCreate(task=task, provider=provider, model=model, session_id=session_id),
    )
    emit(out, lambda a: console.print(f"attempt {a.id} (#{a.number}) {a.status}"))


@attempt_app.command("finish")
@handle_errors
def attempt_finish(
    attempt_id: str,
    status: AttemptStatus = typer.Argument(...),
    summary: str = typer.Option("", "--summary", "-m"),
    error: str | None = typer.Option(None),
) -> None:
    """Finish an externally-run attempt."""
    out = STATE.app().attempts.finish(STATE.ctx(), attempt_id, status, summary=summary, error=error)
    emit(out, lambda a: console.print(f"attempt {short(a.id)} {a.status}"))


@attempt_app.command("compare")
@handle_errors
def attempt_compare(first: str, second: str) -> None:
    """Compare provenance of two attempts (config, inputs, manifests, outcomes)."""
    result = STATE.app().attempts.compare(STATE.ctx(), first, second)
    emit(
        result,
        lambda r: table(
            [(k, v["a"], v["b"], "" if v["same"] else "DIFF") for k, v in r["fields"].items()],
            ["field", "a", "b", ""],
        ),
    )


@attempt_app.command("manifests")
@handle_errors
def attempt_manifests(attempt_id: str) -> None:
    """Exact revision manifests (baseline / outcome / verification) of an attempt."""
    emit(
        STATE.app().attempts.manifests(STATE.ctx(), attempt_id),
        lambda ms: [
            table(
                [
                    (
                        e["repository"],
                        e.get("sha", "")[:12] if e.get("sha") else "",
                        e.get("access"),
                        e.get("checkout_path"),
                    )
                    for e in m["entries"]
                ],
                ["repository", "sha", "access", "path"],
                title=m["kind"],
            )
            for m in ms
        ],
    )


@agent_app.command("list")
@handle_errors
def agent_list() -> None:
    """Configured agent adapters, availability and capabilities."""
    items = STATE.app().agents.describe()
    emit(
        items,
        lambda rows: table(
            [
                (
                    r["name"],
                    r["adapter"],
                    "yes" if r["available"] else "NO",
                    r["detail"][:60],
                    ",".join(k for k, v in r["capabilities"].items() if v),
                )
                for r in rows
            ],
            ["name", "adapter", "available", "detail", "capabilities"],
        ),
    )


@agent_app.command("add-command")
@handle_errors
def agent_add_command(
    name: str,
    executable: str,
    arg: list[str] = typer.Option(None, "--arg", help="Argument template"),
    timeout: float = typer.Option(1800),
    supports_usage: bool = typer.Option(False),
) -> None:
    """Register a generic command agent (workgraph.agent.v1 protocol)."""
    from workgraph.config.settings import CommandAgentConfig
    from workgraph.integrations.agents.command import validate_template

    app = STATE.app()
    validate_template(arg or [])
    config = app.config
    config.agents[name] = CommandAgentConfig(
        executable=executable, args=arg or [], timeout_seconds=timeout, supports_usage=supports_usage
    )
    app.config_store.save_global(config)
    emit(
        {"name": name, "executable": executable, "args": arg or []},
        lambda r: console.print(f"agent {name} configured"),
    )


@run_app.command("start")
@handle_errors
def run_start(
    task: str,
    agent: str | None = typer.Option(None),
    model: str | None = typer.Option(None),
    instructions: str | None = typer.Option(None, help="Extra operator instructions"),
    wait: bool = typer.Option(False, help="Wait for the run to finish (needs a running worker)"),
    workspace: str | None = workspace_option(),
) -> None:
    """Queue an agent run for a task (requires execution to be enabled)."""
    app = STATE.app()
    attempt = app.runs.start(
        STATE.ctx(), ws(workspace), task, agent=agent, model=model, instructions=instructions
    )
    if wait:
        _wait(attempt.id)
        attempt = app.attempts.get(STATE.ctx(), attempt.id)
    emit(attempt, lambda a: console.print(f"attempt {a.id} #{a.number} {a.status}"))


def _wait(attempt_id: str, timeout: float = 7200) -> None:
    app = STATE.app()
    deadline = time.monotonic() + timeout
    last_seq = 0
    while time.monotonic() < deadline:
        run = app.runs.get(STATE.ctx(), attempt_id, events=50)
        for event in run.recent_events:
            if event["seq"] > last_seq:
                last_seq = event["seq"]
                if not STATE.json:
                    console.print(f"[dim]{event['event_type']}[/dim] {event['data'].get('message', '')}")
        if run.attempt.status in TERMINAL_ATTEMPT_STATUSES:
            return
        time.sleep(1)
    fail("timed out waiting for the run", 3)


@run_app.command("get")
@handle_errors
def run_get(attempt_id: str) -> None:
    """Attempt state, job status and recent progress events."""
    run = STATE.app().runs.get(STATE.ctx(), attempt_id)
    emit(
        run,
        lambda r: (
            kv(
                [
                    ("status", r.attempt.status),
                    ("job", r.job_status),
                    ("exit", r.attempt.exit_reason),
                    ("error", r.attempt.error),
                ],
                title=f"attempt {short(r.attempt.id)}",
            ),
            table(
                [(e["seq"], e["event_type"], e["data"].get("message", "")[:80]) for e in r.recent_events],
                ["seq", "event", "message"],
            ),
        ),
    )


@run_app.command("watch")
@handle_errors
def run_watch(attempt_id: str) -> None:
    """Stream progress until the attempt finishes."""
    _wait(attempt_id)
    run_get(attempt_id)


@run_app.command("cancel")
@handle_errors
def run_cancel(attempt_id: str, reason: str = typer.Option("cancelled by operator")) -> None:
    """Cancel a queued or running attempt."""
    emit(
        STATE.app().runs.cancel(STATE.ctx(), attempt_id, reason),
        lambda a: console.print(f"{short(a.id)}: {a.status}"),
    )


@run_app.command("retry")
@handle_errors
def run_retry(attempt_id: str, instructions: str | None = typer.Option(None)) -> None:
    """Start a new attempt after a failed one."""
    emit(
        STATE.app().runs.retry(STATE.ctx(), attempt_id, instructions=instructions),
        lambda a: console.print(f"retry queued as attempt {a.id} #{a.number}"),
    )


@run_app.command("rerun")
@handle_errors
def run_rerun(attempt_id: str) -> None:
    """Re-run with the recorded configuration and baseline revisions (outputs may differ)."""
    result = STATE.app().runs.rerun(STATE.ctx(), attempt_id)
    emit(
        result,
        lambda r: console.print(
            f"rerun queued as {r['attempt']['id']}; baseline reproduced: {r['reproduced_baseline']}"
            + (f"; missing revisions: {r['missing_revisions']}" if r["missing_revisions"] else "")
        ),
    )


@run_app.command("schedule")
@handle_errors
def run_schedule(
    dry_run: bool = typer.Option(False, help="Only list eligible tasks"),
    explain: str | None = typer.Option(None, help="Explain why a task is or is not eligible"),
) -> None:
    """Run one scheduler pass (auto_execute tasks whose dependencies are accepted)."""
    app = STATE.app()
    if explain:
        emit(app.scheduler.explain(STATE.ctx(), STATE.workspace, explain), lambda r: kv(r.items()))
        return
    if dry_run:
        emit(
            app.scheduler.eligible(STATE.ctx()),
            lambda rows: table(
                [(r["key"], r["priority"], r["agent"] or "") for r in rows], ["task", "priority", "agent"]
            ),
        )
        return
    emit(app.scheduler.tick(STATE.ctx()), lambda r: console.print(f"started {len(r['started'])}: {r}"))


@run_app.command("decompose")
@handle_errors
def run_decompose(
    task: str,
    create: bool = typer.Option(False, help="Create the validated children"),
    provider: str | None = typer.Option(None),
    model: str | None = typer.Option(None),
    workspace: str | None = workspace_option(),
) -> None:
    """Propose (and optionally create) child tasks using the configured model."""
    app = STATE.app()
    t = app.tasks.get(STATE.ctx(), ws(workspace), task)
    result = app.scheduler.decompose(STATE.ctx(), t.id, create=create, provider=provider, model=model)
    emit(
        result,
        lambda r: (
            table(
                [(c["ref"], c["title"], ",".join(c["depends_on"])) for c in r["proposal"]["children"]],
                ["ref", "title", "depends on"],
            ),
            console.print(f"created: {r['created']}" if r["created"] else "dry run (use --create)"),
        ),
    )


@verify_app.command("config")
@handle_errors
def verify_config(
    name: str,
    scope_type: str = typer.Option("repository", help="workspace|project|repository|task"),
    scope: str | None = typer.Option(None, help="Scope reference (repository name, project slug, task key)"),
    check: list[str] = typer.Option(
        ..., "--check", help='JSON check, e.g. {"name":"tests","command":["pytest"]}'
    ),
    workspace: str | None = workspace_option(),
) -> None:
    """Create or update an approved verification configuration (trusted commands only)."""
    checks = [parse_json_arg(c, "check") for c in check]
    out = STATE.app().verification.create_config(
        STATE.ctx(), ws(workspace), scope_type=scope_type, scope=scope, name=name, checks=checks
    )
    emit(out, lambda c: console.print(f"verification config {c.name} v{c.version} ({len(c.checks)} checks)"))


@verify_app.command("configs")
@handle_errors
def verify_configs(workspace: str | None = workspace_option()) -> None:
    """List verification configurations."""
    items = STATE.app().verification.list_configs(STATE.ctx(), ws(workspace))
    emit(
        items,
        lambda rows: table(
            [
                (
                    short(c.id),
                    c.name,
                    c.scope_type,
                    short(c.scope_id),
                    len(c.checks),
                    "yes" if c.enabled else "no",
                    c.approved_by,
                )
                for c in rows
            ],
            ["id", "name", "scope", "scope id", "checks", "enabled", "approved by"],
        ),
    )


@verify_app.command("run")
@handle_errors
def verify_run(
    task: str, attempt: str | None = typer.Option(None), workspace: str | None = workspace_option()
) -> None:
    """Run the applicable checks now against the task's current revisions."""
    run = STATE.app().verification.run(STATE.ctx(), ws(workspace), task, attempt_id=attempt)
    emit(
        run,
        lambda r: (
            console.print(f"verification {r.status} (required passed: {r.required_passed})"),
            table(
                [
                    (
                        c.name,
                        c.kind,
                        "req" if c.required else "opt",
                        c.status,
                        c.exit_code,
                        ", ".join(c.failure_signatures)[:60],
                    )
                    for c in r.checks
                ],
                ["check", "kind", "", "status", "exit", "failures"],
            ),
        ),
    )
    if run.status != "passed":
        raise typer.Exit(3)


@verify_app.command("runs")
@handle_errors
def verify_runs(task: str | None = typer.Option(None), workspace: str | None = workspace_option()) -> None:
    """List verification runs."""
    runs = STATE.app().verification.list_runs(STATE.ctx(), ws(workspace), task=task)
    emit(
        runs,
        lambda rs: table(
            [(short(r.id), r.task_id[:8], r.status, len(r.checks), when(r.created_at)) for r in rs],
            ["run", "task", "status", "checks", "created"],
        ),
    )


@verify_app.command("failures")
@handle_errors
def verify_failures(
    check: str | None = typer.Option(None), workspace: str | None = workspace_option()
) -> None:
    """Historical verification failures."""
    rows = STATE.app().verification.failures(STATE.ctx(), ws(workspace), check_name=check)
    emit(
        rows,
        lambda rs: table(
            [
                (
                    r["task_key"],
                    r["check"],
                    r["status"],
                    r["exit_code"],
                    ", ".join(r["failure_signatures"])[:70],
                )
                for r in rs
            ],
            ["task", "check", "status", "exit", "signatures"],
        ),
    )
