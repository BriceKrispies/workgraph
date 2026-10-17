"""backup, restore, export, import and events commands."""

from __future__ import annotations

from pathlib import Path

import typer

from workgraph.cli.common import STATE, console, emit, handle_errors, kv, table, when, workspace_option, ws

export_app = typer.Typer(help="Export workspace records or analytics data.", no_args_is_help=True)
events_app = typer.Typer(help="Inspect the immutable domain event log.", no_args_is_help=True)


@handle_errors
def backup_command(
    output: Path | None = typer.Option(None, "--output", "-o", help="Target .zip (default: data/backups/)"),
    include_artifacts: bool = typer.Option(False, help="Also include run logs and other artifacts"),
) -> None:
    """Create a consistent online backup (database + configuration)."""
    from workgraph.persistence.backup import create_backup

    result = create_backup(STATE.app().paths, output, include_artifacts=include_artifacts)
    emit(result, lambda r: console.print(f"backup written to {r['path']} ({r['bytes']} bytes, schema {r['schema_revision']})"))


@handle_errors
def restore_command(
    archive: Path,
    with_config: bool = typer.Option(False, help="Also restore configuration files"),
    force: bool = typer.Option(False, help="Restore even if a worker appears to be running"),
) -> None:
    """Restore a backup (the current database is kept as a safety copy) and migrate it forward."""
    from workgraph.config.paths import AppPaths
    from workgraph.persistence.backup import restore_backup

    paths = AppPaths.resolve(STATE.home)
    STATE.close()
    result = restore_backup(paths, archive, with_config=with_config, force=force)
    emit(result, lambda r: kv(r.items(), title="restored"))


@export_app.command("workspace")
@handle_errors
def export_workspace(output: Path, workspace: str | None = workspace_option()) -> None:
    """Export all records of a workspace as JSON Lines."""
    from workgraph.persistence.backup import export_workspace as do_export

    app = STATE.app()
    ws_obj = app.workspaces.get(STATE.ctx(), ws(workspace) or app.default_workspace_id())
    result = do_export(app.db, ws_obj.id, output)
    emit(result, lambda r: (console.print(f"exported to {r['path']}"),
                            table(sorted(r["tables"].items()), ["table", "rows"])))


@export_app.command("analytics")
@handle_errors
def export_analytics(output: Path, dataset: str = typer.Option("task_facts"),
                     fmt: str = typer.Option("csv", "--format"), workspace: str | None = workspace_option()) -> None:
    """Export an analytics fact table (task_facts, attempt_facts, task_commits, task_files)."""
    result = STATE.app().analytics.export(STATE.ctx(), ws(workspace), target=output, fmt=fmt, dataset=dataset)
    emit(result, lambda r: console.print(f"wrote {r['path']}"))


@handle_errors
def import_command(source: Path) -> None:
    """Import a workspace export into this database (never overwrites an existing workspace)."""
    from workgraph.persistence.backup import import_workspace
    from workgraph.persistence.orm import Task
    from workgraph.search.indexer import index_task

    app = STATE.app()
    result = import_workspace(app.db, source)
    with app.db.session(write=True) as session:
        from sqlalchemy import select

        for task_id in session.scalars(select(Task.id).where(Task.workspace_id == result["workspace_id"])):
            index_task(session, task_id)
    emit(result, lambda r: (console.print(f"imported workspace {r['workspace_id']}"),
                            table(sorted(r["tables"].items()), ["table", "rows"])))


@events_app.command("list")
@handle_errors
def events_list(entity: str | None = typer.Option(None, help="Entity id"), prefix: str | None = typer.Option(None),
                after: int | None = typer.Option(None, help="Only events after this sequence number"),
                limit: int = typer.Option(50), workspace: str | None = workspace_option()) -> None:
    """List events (filter by entity, type prefix or sequence)."""
    rows = STATE.app().history.events(STATE.ctx(), ws(workspace), entity_id=entity, event_type_prefix=prefix,
                                      after_seq=after, limit=limit, descending=after is None)
    emit(rows, lambda rs: table([(e.seq, when(e.occurred_at), e.event_type, e.actor, e.entity_id[:8]) for e in rs],
                                ["seq", "when", "event", "actor", "entity"]))


@events_app.command("verify")
@handle_errors
def events_verify(workspace: str | None = workspace_option()) -> None:
    """Replay task events and compare with current-state tables."""
    app = STATE.app()
    report = app.history.verify_task_projection(STATE.ctx(), ws(workspace) or app.default_workspace_id())
    emit(report, lambda r: console.print(
        f"{'consistent' if r['consistent'] else 'MISMATCH'}: {r['tasks']} tasks, {r['events']} events"
        + ("" if r["consistent"] else f" {r['mismatches']}")))
    if not report["consistent"]:
        raise typer.Exit(1)
