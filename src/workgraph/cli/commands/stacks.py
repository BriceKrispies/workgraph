"""Stack commands: a parent task's child changes integrated as one unit."""

from __future__ import annotations

from typing import Any

import typer

from workgraph.cli.common import STATE, emit, handle_errors, kv, short, table, workspace_option, ws

stack_app = typer.Typer(
    help="Create and inspect stacks (a parent task's children, integrated on one branch).",
    no_args_is_help=True,
)


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
        [(r.repository_name, short(r.base_sha, 12), r.integration_branch) for r in s.repositories],
        ["repository", "base", "integration branch"],
    )
    table(
        [(i.task_key, i.state, i.merge_order or "", i.detail or "", i.task_title) for i in s.items],
        ["task", "state", "merge order", "detail", "title"],
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


def _render_integration(out: dict[str, Any]) -> None:
    table([(job["repository"], job["job_id"]) for job in out["jobs"]], ["repository", "integration job"])


@stack_app.command("integrate")
@handle_errors
def stack_integrate(parent: str, workspace: str | None = workspace_option()) -> None:
    """Queue merging the stack's ready children onto each repository's integration branch."""
    emit(STATE.app().stacks.integrate(STATE.ctx(), ws(workspace), parent), _render_integration)


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
