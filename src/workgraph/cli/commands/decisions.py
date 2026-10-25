"""Decision commands: questions tasks wait on, with options, and the user's answers."""

from __future__ import annotations

from typing import Any

import typer
from rich.markup import escape

from workgraph.cli.common import (
    STATE,
    console,
    emit,
    fail,
    handle_errors,
    kv,
    table,
    when,
    workspace_option,
    ws,
)
from workgraph.core.models.schemas import DecisionOptionIn

decision_app = typer.Typer(
    help="Structured decisions: a question a task waits on, its options and the answer.",
    no_args_is_help=True,
)


def _answer(d: Any) -> str:
    if d.other_text is not None:
        return f"other: {d.other_text}"
    return d.chosen_option_id or ""


def _render_decision(d: Any) -> None:
    kv(
        [
            ("id", d.id),
            ("task", d.task_key),
            ("status", d.status),
            ("answer", _answer(d) or None),
            ("note", d.note),
            ("decided by", d.decided_by),
            ("decided", when(d.decided_at) if d.decided_at else None),
            ("created", when(d.created_at)),
        ],
        title=f"decision {d.task_key}",
    )
    console.print(f"\n{escape(d.question)}")
    for o in d.options:
        mark = " (recommended)" if o.recommended else ""
        detail = f" - {escape(o.detail)}" if o.detail else ""
        console.print(f"  [bold]{o.id}[/bold] {escape(o.label)}{mark}{detail}")


def _parse_option(spec: str, recommended: bool) -> DecisionOptionIn:
    label, _, detail = spec.partition("::")
    return DecisionOptionIn(label=label.strip(), detail=detail.strip(), recommended=recommended)


@decision_app.command("list")
@handle_errors
def decision_list(
    status: str = typer.Option("open", "--status", help="open, decided, withdrawn or all"),
    limit: int = typer.Option(50, "--limit"),
    workspace: str | None = workspace_option(),
) -> None:
    """List decisions (open ones by task priority)."""
    if status not in ("open", "decided", "withdrawn", "all"):
        fail("--status must be open, decided, withdrawn or all", 2)
    items = STATE.app().decisions.list(STATE.ctx(), ws(workspace), status, limit)  # type: ignore[arg-type]

    def render(rows: list[Any]) -> None:
        table(
            [
                (d.id[:8], d.task_key, d.task.priority, d.status, d.question[:70], _answer(d)[:30])
                for d in rows
            ],
            ["id", "task", "priority", "status", "question", "answer"],
        )

    emit(items, render)


@decision_app.command("show")
@handle_errors
def decision_show(decision_id: str, workspace: str | None = workspace_option()) -> None:
    """Show a decision with its options."""
    emit(
        STATE.app().decisions.get(STATE.ctx(), ws(workspace), _full_id(decision_id, workspace)),
        _render_decision,
    )


@decision_app.command("request")
@handle_errors
def decision_request(
    task: str,
    question: str = typer.Option(..., "--question", "-q"),
    option: list[str] = typer.Option(
        None, "--option", "-o", help="Option as 'label' or 'label::detail' (repeatable)"
    ),
    recommend: int | None = typer.Option(
        None, "--recommend", help="Position (1-based) of the recommended option"
    ),
    workspace: str | None = workspace_option(),
) -> None:
    """Ask a question the task waits on; tags the task needs-decision."""
    specs = option or []
    if recommend is not None and not 1 <= recommend <= len(specs):
        fail(f"--recommend must be between 1 and {len(specs)}", 2)
    options = [_parse_option(spec, recommend == i) for i, spec in enumerate(specs, start=1)]
    out = STATE.app().decisions.request(STATE.ctx(), ws(workspace), task, question, options)
    emit(out, lambda d: console.print(f"decision {d.id} requested on {d.task_key}"))


@decision_app.command("choose")
@handle_errors
def decision_choose(
    decision_id: str,
    option: str | None = typer.Option(None, "--option", "-o", help="Option id"),
    other: str | None = typer.Option(None, "--other", help="Free-text answer instead of an option"),
    note: str | None = typer.Option(None, "--note"),
    workspace: str | None = workspace_option(),
) -> None:
    """Answer a decision (or change its answer)."""
    out = STATE.app().decisions.choose(
        STATE.ctx(),
        ws(workspace),
        _full_id(decision_id, workspace),
        option_id=option,
        other_text=other,
        note=note,
    )
    emit(out, lambda d: console.print(f"decision {d.id} on {d.task_key} decided: {_answer(d)}"))


@decision_app.command("withdraw")
@handle_errors
def decision_withdraw(
    decision_id: str,
    reason: str | None = typer.Option(None, "--reason"),
    workspace: str | None = workspace_option(),
) -> None:
    """Withdraw an open decision that no longer needs an answer."""
    out = STATE.app().decisions.withdraw(STATE.ctx(), ws(workspace), _full_id(decision_id, workspace), reason)
    emit(out, lambda d: console.print(f"decision {d.id} on {d.task_key} withdrawn"))


def _full_id(decision_id: str, workspace: str | None) -> str:
    """Expand the 8-character prefix that ``decision list`` prints to the full id."""
    if len(decision_id) >= 32:
        return decision_id
    matches = [
        d.id
        for d in STATE.app().decisions.list(STATE.ctx(), ws(workspace), "all", 500)
        if d.id.startswith(decision_id)
    ]
    if len(matches) != 1:
        fail(f"decision {decision_id!r} {'is ambiguous' if matches else 'not found'}", 2 if matches else 4)
    return matches[0]
