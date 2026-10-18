"""Experiment commands: a pinned protocol, and the trials measured under it."""

from __future__ import annotations

from typing import Any

import typer

from workgraph.cli.common import (
    STATE,
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
from workgraph.core.errors import ValidationFailed

experiment_app = typer.Typer(
    help="Create, freeze and inspect experiments (pinned protocols measured by trials).",
    no_args_is_help=True,
)
trial_app = typer.Typer(help="Add, inspect and classify the trials of an experiment.", no_args_is_help=True)
replay_app = typer.Typer(
    help="Turn a repository's commits into replay tasks (red tests at the parent, the commit as solution).",
    no_args_is_help=True,
)
experiment_app.add_typer(trial_app, name="trial")
experiment_app.add_typer(replay_app, name="replay")


def _render_experiment(out: dict[str, Any]) -> None:
    exp = out["experiment"]
    kv(
        [
            ("slug", exp.slug),
            ("title", exp.title),
            ("status", exp.status),
            ("question", exp.question or ""),
            ("frozen", when(exp.frozen_at) or "not frozen"),
            ("id", exp.id),
        ],
        title=f"experiment {exp.slug}",
    )
    _render_trials(out["trials"])


def _render_trials(trials: list[Any]) -> None:
    table(
        [
            (
                t.seq,
                t.task_key,
                t.arm,
                t.phase,
                t.band if t.band is not None else "",
                t.status,
                t.outcome or "",
                t.failure_cause or "",
                short(t.actual_model or t.requested_model, 24),
            )
            for t in trials
        ],
        ["seq", "task", "arm", "phase", "band", "status", "outcome", "cause", "model"],
    )


def _render_trial(t: Any) -> None:
    kv(
        [
            ("trial", t.id),
            ("experiment", t.experiment_id),
            ("task", t.task_key),
            ("arm", t.arm),
            ("phase", t.phase),
            ("seq", t.seq),
            ("band", t.band if t.band is not None else ""),
            ("requested model", t.requested_model),
            ("actual model", t.actual_model or ""),
            ("status", t.status),
            ("outcome", t.outcome or ""),
            ("failure cause", f"{t.failure_cause} ({t.failure_cause_source})" if t.failure_cause else ""),
            ("attempt", t.attempt_id or ""),
            ("subagent run", t.subagent_run_id or ""),
            ("base ref", t.base_ref or ""),
            ("worktree", t.worktree_path or ""),
            ("notes", t.notes or ""),
        ],
        title=f"trial {t.seq} of {t.task_key}",
    )


@experiment_app.command("create")
@handle_errors
def experiment_create(
    slug: str,
    title: str = typer.Option(..., "--title", help="Human-readable title"),
    question: str | None = typer.Option(None, "--question", help="The question the experiment answers"),
    protocol: str = typer.Option(..., "--protocol", help="Protocol JSON, or @file.json"),
    workspace: str | None = workspace_option(),
) -> None:
    """Create a draft experiment. Its protocol can change until it is frozen."""
    spec = parse_json_arg(protocol, "protocol")
    emit(
        STATE.app().experiments.create(STATE.ctx(), ws(workspace), slug, title, question, spec),
        lambda e: kv(
            [("slug", e.slug), ("status", e.status), ("id", e.id)], title=f"created experiment {e.slug}"
        ),
    )


@experiment_app.command("list")
@handle_errors
def experiment_list(
    status: str | None = typer.Option(None, "--status", help="draft, running or closed"),
    workspace: str | None = workspace_option(),
) -> None:
    """List experiments in the workspace."""
    rows = STATE.app().experiments.list(STATE.ctx(), ws(workspace), status=status)
    emit(
        rows,
        lambda items: table(
            [(e.slug, e.status, when(e.frozen_at), e.title) for e in items],
            ["slug", "status", "frozen", "title"],
        ),
    )


@experiment_app.command("show")
@handle_errors
def experiment_show(experiment: str, workspace: str | None = workspace_option()) -> None:
    """Show an experiment's protocol summary and its trials in order."""
    app = STATE.app()
    ctx = STATE.ctx()
    out = {
        "experiment": app.experiments.get(ctx, ws(workspace), experiment),
        "trials": app.experiments.trials(ctx, ws(workspace), experiment),
    }
    emit(out, _render_experiment)


@experiment_app.command("freeze")
@handle_errors
def experiment_freeze(experiment: str, workspace: str | None = workspace_option()) -> None:
    """Validate the protocol, freeze it and start the experiment."""
    emit(
        STATE.app().experiments.freeze(STATE.ctx(), ws(workspace), experiment),
        lambda e: kv(
            [("slug", e.slug), ("status", e.status), ("frozen", when(e.frozen_at))], title=f"frozen {e.slug}"
        ),
    )


@experiment_app.command("close")
@handle_errors
def experiment_close(experiment: str, workspace: str | None = workspace_option()) -> None:
    """Close a running experiment; its trials no longer change."""
    emit(
        STATE.app().experiments.close(STATE.ctx(), ws(workspace), experiment),
        lambda e: kv([("slug", e.slug), ("status", e.status)], title=f"closed {e.slug}"),
    )


@trial_app.command("add")
@handle_errors
def trial_add(
    experiment: str,
    task: str,
    arm: str = typer.Option(..., "--arm", help="An arm named in the protocol"),
    phase: str = typer.Option(..., "--phase", help="staircase, refine, control, repeat or live"),
    band: int | None = typer.Option(None, "--band", help="Size band, within the protocol's range"),
    workspace: str | None = workspace_option(),
) -> None:
    """Add a planned trial of a task to a running experiment."""
    emit(
        STATE.app().experiments.add_trial(STATE.ctx(), ws(workspace), experiment, task, arm, phase, band),
        _render_trial,
    )


@trial_app.command("show")
@handle_errors
def trial_show(trial: str, workspace: str | None = workspace_option()) -> None:
    """Show one trial: its arm, models, status, outcome and failure cause."""
    emit(STATE.app().experiments.trial(STATE.ctx(), ws(workspace), trial), _render_trial)


@trial_app.command("classify")
@handle_errors
def trial_classify(
    trial: str,
    cause: str = typer.Option(..., "--cause", help="Failure cause, e.g. too_big or missing_context"),
    source: str = typer.Option(..., "--source", help="human, agent or mechanical"),
    notes: str | None = typer.Option(None, "--notes", help="Free-text note"),
    workspace: str | None = workspace_option(),
) -> None:
    """Record why a failed or invalid trial failed."""
    emit(
        STATE.app().experiments.classify(STATE.ctx(), ws(workspace), trial, cause, source, notes),
        _render_trial,
    )


@replay_app.command("scan")
@handle_errors
def replay_scan(
    repo: str = typer.Option(..., "--repo", help="Repository name or id"),
    rev: str = typer.Option("HEAD", "--rev", help="Revision to walk back from (newest first)"),
    limit: int | None = typer.Option(None, "--limit", help="Stop after this many candidates"),
    min_lines: int = typer.Option(1, "--min-lines", help="Smallest non-test source size, in lines"),
    max_lines: int | None = typer.Option(None, "--max-lines", help="Largest non-test source size, in lines"),
    include_excluded: bool = typer.Option(False, "--include-excluded", help="Also list excluded commits"),
    workspace: str | None = workspace_option(),
) -> None:
    """List commits that could become replay tasks, with their size band and exclusion reasons."""
    candidates = STATE.app().replay_pool.scan(
        STATE.ctx(),
        ws(workspace),
        repo,
        rev=rev,
        limit=limit,
        min_lines=min_lines,
        max_lines=max_lines,
        include_excluded=include_excluded,
    )
    emit(
        [c.as_dict() for c in candidates],
        lambda items: table(
            [
                (
                    c.sha[:12],
                    c.features.band if c.features.band is not None else "",
                    c.features.src_lines,
                    c.features.test_lines,
                    c.features.src_files,
                    ",".join(c.features.packages),
                    ",".join(c.excluded),
                    short(c.subject, 60),
                )
                for c in candidates
            ],
            ["sha", "band", "src_lines", "test_lines", "src_files", "packages", "excluded", "subject"],
        ),
    )


@replay_app.command("create")
@handle_errors
def replay_create(
    shas: list[str] = typer.Argument(..., help="Commit shas to turn into replay tasks"),
    repo: str = typer.Option(..., "--repo", help="Repository name or id"),
    validate: bool = typer.Option(
        False, "--validate", help="Run the oracle at the base and the commit first"
    ),
    oracle_template: str | None = typer.Option(
        None,
        "--oracle-template",
        help='Oracle argv as a JSON list, e.g. ["python", "-m", "pytest", "{test_files}"]',
    ),
    env: list[str] | None = typer.Option(None, "--env", help="K=V passed to oracle commands (repeatable)"),
    timeout: float = typer.Option(1800.0, "--timeout", help="Seconds allowed per oracle command"),
    workspace: str | None = workspace_option(),
) -> None:
    """Create one replay task per commit. Existing replay tasks for a commit are reported, not duplicated."""
    template = parse_json_arg(oracle_template, "oracle template")
    if template is not None and not (
        isinstance(template, list) and all(isinstance(t, str) for t in template)
    ):
        raise ValidationFailed("--oracle-template must be a JSON list of strings")
    environment: dict[str, str] = {}
    for item in env or []:
        key, sep, value = item.partition("=")
        if not sep or not key:
            raise ValidationFailed(f"--env expects K=V, got {item!r}")
        environment[key] = value
    results = STATE.app().replay_pool.create(
        STATE.ctx(),
        ws(workspace),
        repo,
        shas,
        validate=validate,
        oracle_template=template,
        env=environment,
        timeout=timeout,
    )
    emit(
        results,
        lambda items: table(
            [(r["sha"][:12], r["status"], r["task_key"] or "", r["reason"] or "") for r in items],
            ["sha", "status", "task", "reason"],
        ),
    )
