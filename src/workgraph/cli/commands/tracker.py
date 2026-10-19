"""tracker commands: read-only sync of issue trackers (Jira) into tasks."""

from __future__ import annotations

from typing import Any

import typer

from workgraph.cli.common import STATE, console, emit, handle_errors, kv, table

tracker_app = typer.Typer(help="Issue tracker connections (read-only Jira sync).", no_args_is_help=True)


@tracker_app.command("list")
@handle_errors
def tracker_list() -> None:
    """Configured tracker connections."""
    config = STATE.app().config
    rows = [
        {
            "name": name,
            "flavour": t.flavour,
            "base_url": t.base_url,
            "workspace": t.workspace,
            "projects": list(t.projects),
        }
        for name, t in sorted(config.trackers.items())
    ]
    emit(
        rows,
        lambda rs: table(
            [(r["name"], r["flavour"], r["base_url"], r["workspace"], ",".join(r["projects"])) for r in rs],
            ["name", "flavour", "base_url", "workspace", "projects"],
        ),
    )


@tracker_app.command("sync")
@handle_errors
def tracker_sync(
    connection: str,
    full: bool = typer.Option(
        False, "--full", help="Every issue in scope, not only those changed since the cursor"
    ),
    queue: bool = typer.Option(False, "--queue", help="Enqueue the sync as a job instead of running it now"),
) -> None:
    """Mirror a connection's issues into tasks, now or as a queued job."""
    app = STATE.app()
    if queue:
        from workgraph.jobs.trackers import enqueue_tracker_sync

        job_id = enqueue_tracker_sync(app, STATE.ctx(), connection, full=full)
        emit(
            {"job_id": job_id, "connection": connection, "full": full},
            lambda r: console.print(f"enqueued {job_id}"),
        )
        return
    report = app.trackers.sync(STATE.ctx(), connection, full=full).as_dict()

    def human(r: dict[str, Any]) -> None:
        kv(
            [
                ("connection", r["connection"]),
                ("full", r["full"]),
                ("fetched", r["fetched"]),
                ("created", r["created"]),
                ("updated", r["updated"]),
                ("unchanged", r["unchanged"]),
                ("missing", r["missing"]),
                ("archived", r["archived"]),
                ("unresolved pull requests", r["unresolved_pull_requests"]),
                ("errors", r["errors"]),
                ("cursor", r["cursor"]),
            ],
            title="tracker sync",
        )

    emit(report, human)


@tracker_app.command("status")
@handle_errors
def tracker_status(connection: str | None = typer.Argument(None, help="Connection (default: all)")) -> None:
    """Cursor, last sync, tracked task counts and unresolved pull request repositories."""
    app = STATE.app()
    if connection:
        emit(app.trackers.status(STATE.ctx(), connection), _status_human)
        return
    statuses = [app.trackers.status(STATE.ctx(), name) for name in sorted(app.config.trackers)]

    def human(items: list[dict[str, Any]]) -> None:
        for status in items:
            _status_human(status)

    emit(statuses, human)


def _status_human(status: dict[str, Any]) -> None:
    kv(list(status.items()), title=f"tracker {status.get('connection', '')}".strip())
