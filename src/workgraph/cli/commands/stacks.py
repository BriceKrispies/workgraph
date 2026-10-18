"""Stack commands: a parent task's child changes integrated as one unit."""

from __future__ import annotations

import json
from typing import Any

import typer

from workgraph.cli.common import (
    STATE,
    console,
    emit,
    handle_errors,
    kv,
    short,
    table,
    when,
    workspace_option,
    ws,
)

stack_app = typer.Typer(
    help="Create and inspect stacks (a parent task's children, integrated on one branch).",
    no_args_is_help=True,
)


def _detail_summary(text: str | None) -> str:
    """One line for an item's detail: its conflicting paths and its ejection, when it has either."""
    try:
        parsed = json.loads(text) if text else {}
    except json.JSONDecodeError:
        return text or ""
    if not isinstance(parsed, dict):
        return text or ""
    parts = [
        f"conflict in {repo}: {', '.join(conflict.get('paths') or []) or 'no paths reported'}"
        for repo, conflict in (parsed.get("conflicts") or {}).items()
    ]
    ejection = parsed.get("ejection")
    if ejection:
        failed = ", ".join(ejection.get("failed_checks") or [])
        parts.append(f"ejected at position {ejection.get('position')}: {failed}")
    return "; ".join(parts)


def _render_stack(s: Any) -> None:
    kv(
        [
            ("parent", s.parent_key),
            ("title", s.parent_title),
            ("status", s.status),
            ("id", s.id),
        ],
        title=f"stack {s.parent_key}",
    )
    table(
        [
            (r.repository_name, short(r.base_sha, 12), r.integration_branch, short(r.integration_tip, 12))
            for r in s.repositories
        ],
        ["repository", "base", "integration branch", "tip"],
    )
    table(
        [
            (i.merge_order or "", i.task_key, i.state, _detail_summary(i.detail), i.task_title)
            for i in s.items
        ],
        ["merge order", "task", "state", "detail", "title"],
    )
    if s.status == "verified":
        unfinished = [f"{i.task_key} ({i.state})" for i in s.items if i.state != "verified"]
        if unfinished:
            console.print(
                f"Verified on the integration tip, but it cannot land while children are unfinished: "
                f"{', '.join(unfinished)}. Finish them or remove them with: workgraph stack remove {s.parent_key} <TASK>"
            )
        else:
            console.print(
                f"Verified on the integration tip. Land it with: workgraph task accept {s.parent_key}"
            )


@stack_app.command("create")
@handle_errors
def stack_create(parent: str, workspace: str | None = workspace_option()) -> None:
    """Stack a parent task's children, branching from each repository's default branch now."""
    emit(STATE.app().stacks.create(STATE.ctx(), ws(workspace), parent), _render_stack)


@stack_app.command("show")
@handle_errors
def stack_show(parent: str, workspace: str | None = workspace_option()) -> None:
    """Show the most recent stack of a parent task."""
    emit(STATE.app().stacks.get(STATE.ctx(), ws(workspace), parent), _render_stack)


@stack_app.command("add")
@handle_errors
def stack_add(parent: str, task: str, workspace: str | None = workspace_option()) -> None:
    """Add a child task of the parent to its active stack."""
    emit(STATE.app().stacks.add_item(STATE.ctx(), ws(workspace), parent, task), _render_stack)


@stack_app.command("remove")
@handle_errors
def stack_remove(parent: str, task: str, workspace: str | None = workspace_option()) -> None:
    """Remove a child task from the active stack (not once it is merged or verified)."""
    emit(STATE.app().stacks.remove_item(STATE.ctx(), ws(workspace), parent, task), _render_stack)


def _event_summary(event: Any) -> str:
    keys = ("task_key", "from", "to", "position", "passed", "run_id", "verification_run_id", "failed_checks")
    return ", ".join(
        f"{key}={event.data[key]}" for key in keys if event.data.get(key) not in (None, "", [], {})
    )


def _render_history(events: list[Any]) -> None:
    table(
        [(e.seq, when(e.occurred_at), e.event_type, _event_summary(e)) for e in events],
        ["seq", "time", "event", "detail"],
    )


@stack_app.command("history")
@handle_errors
def stack_history(parent: str, workspace: str | None = workspace_option()) -> None:
    """Show the stack's events in order: merges, conflicts, verification, bisection, rebuilds and landing."""
    emit(STATE.app().stacks.history(STATE.ctx(), ws(workspace), parent), _render_history)


def _render_integration(out: dict[str, Any]) -> None:
    table([(job["repository"], job["job_id"]) for job in out["jobs"]], ["repository", "integration job"])


@stack_app.command("integrate")
@handle_errors
def stack_integrate(parent: str, workspace: str | None = workspace_option()) -> None:
    """Queue merging the stack's ready children onto each repository's integration branch."""
    emit(STATE.app().stacks.integrate(STATE.ctx(), ws(workspace), parent), _render_integration)


def _render_verification(out: dict[str, Any]) -> None:
    table([(out["stack"], out["job_id"])], ["stack", "verification job"])


@stack_app.command("verify")
@handle_errors
def stack_verify(parent: str, workspace: str | None = workspace_option()) -> None:
    """Queue full verification of the stack's integration tip (bisecting and ejecting on failure)."""
    emit(STATE.app().stack_verification.request(STATE.ctx(), ws(workspace), parent), _render_verification)


@stack_app.command("list")
@handle_errors
def stack_list(
    workspace: str | None = workspace_option(),
    all_: bool = typer.Option(False, "--all", help="Include landed and abandoned stacks"),
) -> None:
    """List stacks in the workspace."""
    items = STATE.app().stacks.list(STATE.ctx(), ws(workspace), include_inactive=all_)
    emit(
        items,
        lambda rows: table(
            [(s.parent_key, s.status, len(s.repositories), len(s.items), s.parent_title) for s in rows],
            ["parent", "status", "repositories", "tasks", "title"],
        ),
    )
