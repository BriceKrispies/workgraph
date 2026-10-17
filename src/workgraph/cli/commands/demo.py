"""demo commands: the end-to-end demonstration and UI demo data."""

from __future__ import annotations

import tempfile
from pathlib import Path

import typer

from workgraph.cli.common import STATE, console, emit, fail, handle_errors

demo_app = typer.Typer(help="Offline demonstrations and demo data (never run implicitly).", no_args_is_help=True)


@demo_app.command("e2e")
@handle_errors
def demo_e2e(home: Path | None = typer.Option(None, help="Directory for the demo's isolated Workgraph home"),
             keep: bool = typer.Option(False, help="Keep the demo home afterwards")) -> None:
    """Run the 25-step cross-repository demonstration in an isolated home (offline)."""
    from workgraph.demo.scenario import run_demo

    temp = None
    if home is None:
        temp = tempfile.mkdtemp(prefix="workgraph-demo-")
        home = Path(temp)
    from workgraph.demo.scenario import DemoFailure

    try:
        report = run_demo(home, log=(lambda _m: None) if STATE.json else console.print)
    except DemoFailure as exc:
        fail(f"demonstration failed: {exc} (home kept at {home})", 1)
    emit({"ok": report.ok, "home": report.home, "steps": report.steps},
         lambda r: console.print(f"[green]demonstration passed ({len(r['steps'])} steps)[/green]; home: {r['home']}"))
    if temp and not keep:
        import shutil

        shutil.rmtree(temp, ignore_errors=True)
    if not report.ok:
        raise typer.Exit(1)


@demo_app.command("seed")
@handle_errors
def demo_seed(workspace: str = typer.Option("demo", help="Workspace slug to create"),
              force: bool = typer.Option(False, help="Allow seeding into an existing home with data")) -> None:
    """Populate demo data (real repos, tasks, runs, analytics) for exploring the web UI."""
    from workgraph.demo.seed import seed_demo

    app = STATE.app()
    existing = [w.slug for w in app.workspaces.list(STATE.ctx())]
    if workspace in existing:
        fail(f"workspace {workspace!r} already exists", 3)
    if existing and not force:
        fail("this Workgraph home already has data; use --force or --home <dir> for an isolated demo", 3)
    summary = seed_demo(app, workspace, log=console.print if not STATE.json else (lambda _m: None))
    emit(summary, lambda s: console.print(f"[green]demo data ready[/green] in workspace {s['workspace']}: "
                                          f"{s['tasks']} tasks, {s['attempts']} attempts, {s['commits']} commits"))
