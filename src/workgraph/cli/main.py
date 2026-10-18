"""The ``workgraph`` command-line interface."""

from __future__ import annotations

import sys
from pathlib import Path

import typer

from workgraph import __version__
from workgraph.cli.commands import core, repo, task
from workgraph.cli.common import STATE

app = typer.Typer(
    name="workgraph",
    help="Workgraph - local-first engineering control plane for agents and humans.",
    no_args_is_help=True,
    pretty_exceptions_enable=False,
    add_completion=False,
)


def _version(value: bool) -> None:
    if value:
        print(f"workgraph {__version__}")
        raise typer.Exit


@app.callback()
def _root(
    home: Path | None = typer.Option(
        None, "--home", envvar="WORKGRAPH_HOME", help="Relocate all Workgraph state"
    ),
    workspace: str | None = typer.Option(
        None, "--workspace", "-w", envvar="WORKGRAPH_WORKSPACE", help="Default workspace for this invocation"
    ),
    version: bool = typer.Option(False, "--version", callback=_version, is_eager=True),
) -> None:
    """Global options. Pass --json anywhere for machine-readable output."""
    STATE.home = home
    STATE.workspace = workspace


app.command("init")(core.init_command)
app.command("doctor")(core.doctor_command)
app.add_typer(core.config_app, name="config")
app.add_typer(core.workspace_app, name="workspace")
app.add_typer(core.project_app, name="project")
app.add_typer(repo.repo_app, name="repo")
app.add_typer(task.task_app, name="task")


def _register_optional_groups() -> None:
    """Command groups that live in their own modules."""
    from workgraph.cli.commands import (
        agents,
        analytics,
        automation,
        claude,
        data,
        demo,
        experiments,
        gitcmd,
        intelligence,
        jobs,
        servers,
        stacks,
        ui,
    )

    app.add_typer(agents.attempt_app, name="attempt")
    app.add_typer(agents.agent_app, name="agent")
    app.add_typer(agents.run_app, name="run")
    app.add_typer(agents.verify_app, name="verify")
    app.add_typer(jobs.worker_app, name="worker")
    app.add_typer(jobs.job_app, name="job")
    app.add_typer(gitcmd.git_app, name="git")
    app.add_typer(stacks.stack_app, name="stack")
    app.add_typer(experiments.experiment_app, name="experiment")
    app.add_typer(intelligence.classify_app, name="classify")
    app.command("search")(intelligence.search_command)
    app.add_typer(analytics.analytics_app, name="analytics")
    app.add_typer(automation.automation_app, name="automation")
    app.add_typer(claude.claude_app, name="claude")
    app.command("serve")(servers.serve_command)
    app.command("mcp")(servers.mcp_command)
    app.command("backup")(data.backup_command)
    app.command("restore")(data.restore_command)
    app.add_typer(data.export_app, name="export")
    app.command("import")(data.import_command)
    app.add_typer(data.events_app, name="events")
    app.add_typer(demo.demo_app, name="demo")
    app.add_typer(ui.ui_app, name="ui")


_register_optional_groups()


def main() -> None:
    argv = sys.argv[1:]
    # Fast path for Claude Code hooks: avoid importing anything heavy.
    if argv[:2] == ["claude", "hook"]:
        from workgraph.integrations.claude_code.hook_cli import run_hook

        raise SystemExit(run_hook(argv[2:]))
    if "--json" in argv:
        STATE.json = True
        sys.argv = [sys.argv[0], *[a for a in argv if a != "--json"]]
    try:
        app()
    finally:
        STATE.close()


if __name__ == "__main__":
    main()
