"""analytics commands."""

from __future__ import annotations

from datetime import datetime
from pathlib import Path
from typing import Any, cast

import typer

from workgraph.analytics.service import AnalyticsFilters, AnalyticsQuery
from workgraph.cli.common import STATE, console, emit, handle_errors, kv, table, workspace_option, ws

analytics_app = typer.Typer(
    help="Engineering analytics (DuckDB over a read-only snapshot).", no_args_is_help=True
)


def _filters(
    project: str | None,
    repository: str | None,
    category: str | None,
    provider: str | None,
    model: str | None,
    since: datetime | None,
    until: datetime | None,
) -> AnalyticsFilters:
    return AnalyticsFilters(
        project=project,
        repository=repository,
        category=category,
        provider=provider,
        model=model,
        since=since,
        until=until,
    )


@analytics_app.command("metrics")
@handle_errors
def metrics() -> None:
    """Catalog of metrics with their definitions."""
    defs = STATE.app().analytics.definitions()
    emit(
        defs,
        lambda rows: table(
            [(d["id"], d["family"], d["statistic"], d["unit"], d["definition"][:80]) for d in rows],
            ["metric", "family", "stat", "unit", "definition"],
        ),
    )


@analytics_app.command("query")
@handle_errors
def query(
    metric: str,
    group_by: list[str] = typer.Option(
        None, "--by", help="period, project, repository, category, provider, model, ..."
    ),
    period: str = typer.Option("week"),
    project: str | None = typer.Option(None, "--project", "-p"),
    repository: str | None = typer.Option(None, "--repo", "-r"),
    category: str | None = typer.Option(None),
    provider: str | None = typer.Option(None),
    model: str | None = typer.Option(None),
    since: datetime | None = typer.Option(None),
    until: datetime | None = typer.Option(None),
    workspace: str | None = workspace_option(),
) -> None:
    """Run a metric with grouping and filters."""
    result = STATE.app().analytics.query(
        STATE.ctx(),
        ws(workspace),
        AnalyticsQuery(
            metric=metric,
            group_by=group_by or [],
            period=cast(Any, period),
            filters=_filters(project, repository, category, provider, model, since, until),
        ),
    )

    def human(r: dict[str, Any]) -> None:
        console.print(f"[bold]{r['metric']['title']}[/bold] - {r['metric']['definition']}")
        rows = r["rows"]
        if rows:
            cols = [c for c in rows[0] if not c.endswith("_label")]
            table([[row.get(f"{c}_label", row.get(c)) for c in cols] for row in rows], cols)
        else:
            console.print("[dim]no data[/dim]")
        for note in r["notes"]:
            console.print(f"[dim]note: {note}[/dim]")

    emit(result, human)


@analytics_app.command("overview")
@handle_errors
def overview(period: str = typer.Option("week"), workspace: str | None = workspace_option()) -> None:
    """Dashboard bundle of the most common metrics."""
    emit(
        STATE.app().analytics.overview(STATE.ctx(), ws(workspace), period=period),
        lambda r: [kv([(k, v)], title=None) for k, v in r.items()],
    )


@analytics_app.command("compare")
@handle_errors
def compare(
    metric: str,
    a_since: datetime,
    a_until: datetime,
    b_since: datetime,
    b_until: datetime,
    group_by: list[str] = typer.Option(None, "--by"),
    workspace: str | None = workspace_option(),
) -> None:
    """Compare a metric between two periods."""
    result = STATE.app().analytics.compare(
        STATE.ctx(),
        ws(workspace),
        metric,
        a=(a_since, a_until),
        b=(b_since, b_until),
        group_by=group_by or [],
    )
    emit(
        result,
        lambda r: table(
            [
                (
                    *(row.get(g) for g in (group_by or [])),
                    row["a"],
                    row["b"],
                    row["delta"],
                    row["n_a"],
                    row["n_b"],
                )
                for row in r["rows"]
            ],
            [*(group_by or []), "a", "b", "delta", "n_a", "n_b"],
        ),
    )


@analytics_app.command("coupling")
@handle_errors
def coupling(
    project: str | None = typer.Option(None),
    min_support: int = typer.Option(1),
    workspace: str | None = workspace_option(),
) -> None:
    """Repository co-change matrix, declared vs observed relationships and coupling candidates."""
    result = STATE.app().analytics.coupling(
        STATE.ctx(), ws(workspace), project=project, min_support=min_support
    )

    def human(r: dict[str, Any]) -> None:
        kv(r["summary"].items(), title="coordination")
        table(
            [
                (
                    e["source_name"],
                    e["target_name"],
                    e["co_changes"],
                    e["jaccard"],
                    e["relationship"],
                    ", ".join(ev["task_key"] for ev in e["evidence"][:5]),
                )
                for e in r["edges"]
            ],
            ["repository", "repository", "co-changes", "jaccard", "relationship", "evidence"],
        )
        for note in r["notes"]:
            console.print(f"[dim]{note}[/dim]")

    emit(result, human)


@analytics_app.command("subagents")
@handle_errors
def subagents(
    period: str = typer.Option("week", help="week or month"),
    since: datetime | None = typer.Option(None, help="Inclusive start date (UTC)"),
    until: datetime | None = typer.Option(None, help="Exclusive end date (UTC)"),
    model: str | None = typer.Option(None, "--model", help="Only runs of this model"),
    workspace: str | None = workspace_option(),
) -> None:
    """Subagent runs by period and model, with linked task outcomes and task size bands."""
    result = STATE.app().subagent_report.report(
        STATE.ctx(),
        ws(workspace),
        period=cast(Any, period),
        since=since,
        until=until,
        model=model,
    )

    def human(r: dict[str, Any]) -> None:
        table(
            [
                (
                    row["period"],
                    row["model"],
                    row["runs"],
                    row["linked_runs"],
                    row["tasks"],
                    row["accepted"],
                    row["rejected"],
                    row["pending"],
                    _rate_text(row["acceptance_rate"], row["wilson"]),
                    _tokens_text(row["median_output_tokens"]),
                )
                for row in r["by_period"]
            ],
            [
                "period",
                "model",
                "runs",
                "linked",
                "tasks",
                "accepted",
                "rejected",
                "pending",
                "rate",
                "median tokens",
            ],
            title="subagent work by period",
        )
        table(
            [
                (
                    row["model"],
                    "unknown" if row["band"] is None else f"2^{row['band']}",
                    row["tasks"],
                    f"{row['accepted']}/{row['decided']}",
                    _rate_text(row["acceptance_rate"], row["wilson"]),
                )
                for row in r["by_size"]
            ],
            ["model", "size (lines)", "tasks", "accepted/decided", "rate"],
            title="acceptance by task size",
        )
        for note in r["notes"]:
            console.print(f"[dim]note: {note}[/dim]")

    emit(result, human)


@analytics_app.command("time")
@handle_errors
def agent_time(
    since: datetime | None = typer.Option(None, help="Inclusive start date (UTC)"),
    until: datetime | None = typer.Option(None, help="Exclusive end date (UTC)"),
    by: str = typer.Option("class", "--by", help="class, model, task or description"),
    model: str | None = typer.Option(None, "--model", help="Only runs of this model"),
    workspace: str | None = workspace_option(),
) -> None:
    """Where subagent wall time went: share by tool class, or per model, task or agent description."""
    result = STATE.app().agent_time.breakdown(
        STATE.ctx(), ws(workspace), since=since, until=until, by=cast(Any, by), model=model
    )

    def human(r: dict[str, Any]) -> None:
        totals = r["totals"]
        console.print(
            f"{totals['runs']} runs, {totals['accounted_seconds'] / 3600:.2f} agent-hours accounted "
            f"(model {_share_text(totals['model_share'])}, waiting {_share_text(totals['waiting_share'])}, "
            f"polling {_share_text(totals['polling_share'])})"
        )
        if r["by"] == "class":
            table(
                [
                    (
                        row["class"],
                        row["kind"],
                        f"{row['seconds'] / 60:.1f}",
                        row["calls"],
                        _share_text(row["share"]),
                    )
                    for row in r["rows"]
                ],
                ["class", "kind", "minutes", "calls", "share"],
                title="subagent time by class",
            )
        else:
            table(
                [
                    (
                        row["key"],
                        row["runs"],
                        f"{row['accounted_seconds'] / 60:.1f}",
                        f"{row['waiting_seconds'] / 60:.1f}",
                        _share_text(row["polling_share"]),
                        ", ".join(f"{c['class']} {c['seconds'] / 60:.1f}m" for c in row["top_classes"]),
                    )
                    for row in r["rows"]
                ],
                [by, "runs", "minutes", "waiting min", "polling", "top classes"],
                title=f"subagent time by {by}",
            )
        for note in r["notes"]:
            console.print(f"[dim]note: {note}[/dim]")

    emit(result, human)


@analytics_app.command("bottlenecks")
@handle_errors
def bottlenecks(
    since: datetime | None = typer.Option(None, help="Inclusive start date (UTC)"),
    until: datetime | None = typer.Option(None, help="Exclusive end date (UTC)"),
    top: int = typer.Option(15, help="How many slowest calls and tasks to show"),
    workspace: str | None = workspace_option(),
) -> None:
    """What slows subagents down: time by class, the slowest calls with live-agent concurrency, and per-task wall time."""
    result = STATE.app().agent_time.bottlenecks(STATE.ctx(), ws(workspace), since=since, until=until, top=top)

    def human(r: dict[str, Any]) -> None:
        totals = r["totals"]
        console.print(
            f"{totals['runs']} runs, {totals['span_seconds'] / 3600:.2f} agent-hours of wall time; "
            f"peak concurrency {r['concurrency']['peak_during_window']} agents; "
            f"waiting {_share_text(totals['waiting_share'])} and polling {_share_text(totals['polling_share'])} "
            "of accounted time"
        )
        table(
            [
                (row["class"], row["kind"], f"{row['seconds'] / 60:.1f}", _share_text(row["share"]))
                for row in r["classes"]
            ],
            ["class", "kind", "minutes", "share"],
            title="share of time by class",
        )
        table(
            [
                (
                    f"{call['seconds']:.0f}",
                    call["class"],
                    "-" if call["concurrent_runs"] is None else call["concurrent_runs"],
                    call["description"],
                    (call["command"] or "")[:70],
                )
                for call in r["slowest_calls"]
            ],
            ["seconds", "class", "concurrent", "agent", "call"],
            title="slowest calls",
        )
        table(
            [
                (
                    row["key"],
                    row["runs"],
                    f"{row['span_seconds'] / 60:.1f}",
                    ", ".join(f"{c['class']} {c['seconds'] / 60:.1f}m" for c in row["top_classes"]),
                )
                for row in r["by_task"]
            ],
            ["task", "runs", "wall min", "top classes"],
            title="agent wall time by task",
        )
        for note in r["notes"]:
            console.print(f"[dim]note: {note}[/dim]")

    emit(result, human)


def _share_text(share: float | None) -> str:
    return "-" if share is None else f"{share * 100:.1f}%"


def _rate_text(rate: float | None, interval: list[float] | None) -> str:
    if rate is None or interval is None:
        return "-"
    return f"{rate:.2f} [{interval[0]:.2f}, {interval[1]:.2f}]"


def _tokens_text(value: float | None) -> str:
    return "-" if value is None else f"{value:.0f}"


@analytics_app.command("failures")
@handle_errors
def failures(
    persist: bool = typer.Option(False, help="Store findings"), workspace: str | None = workspace_option()
) -> None:
    """Recurring failure patterns with linked evidence."""
    app = STATE.app()
    if persist:
        from workgraph.analytics.failures import persist_findings

        persist_findings(app, ws(workspace) or app.default_workspace_id())
    result = app.analytics.failures(STATE.ctx(), ws(workspace))
    emit(
        result,
        lambda r: table(
            [(f["kind"], f["severity"], f["title"], f["sample_size"]) for f in r["findings"]],
            ["kind", "severity", "finding", "n"],
        ),
    )


@analytics_app.command("estimate")
@handle_errors
def estimate(
    task: str | None = typer.Argument(None),
    title: str | None = typer.Option(None),
    category: str | None = typer.Option(None),
    repo: list[str] = typer.Option(None, "--repo"),
    workspace: str | None = workspace_option(),
) -> None:
    """Estimate from comparable historical tasks (or report insufficient data)."""
    result = STATE.app().analytics.estimate(
        STATE.ctx(), ws(workspace), task=task, title=title, category=category, repositories=repo or []
    )

    def human(r: dict[str, Any]) -> None:
        if r["status"] != "ok":
            console.print(f"[yellow]{r['message']}[/yellow]")
            return
        console.print(f"based on {r['sample_size']} comparable tasks ({r['method']} similarity)")
        table(
            [
                (name, d["n"], d["p25"], d["median"], d["p75"], d["p90"])
                for name, d in r["distributions"].items()
            ],
            ["measure", "n", "p25", "median", "p75", "p90"],
        )
        table([(b["key"], b["score"], "; ".join(b["reasons"])) for b in r["basis"]], ["task", "score", "why"])
        table(
            [
                (a["provider"], a["model"], a["attempts"], a["success_rate"])
                for a in r["agent_recommendations"]
            ],
            ["provider", "model", "attempts", "success"],
            title="agents on similar tasks (selection bias applies)",
        )

    emit(result, human)


@analytics_app.command("history")
@handle_errors
def history(
    category: str | None = typer.Option(None),
    repo: str | None = typer.Option(None, "--repo"),
    limit: int = typer.Option(30),
    workspace: str | None = workspace_option(),
) -> None:
    """Historical accepted tasks with their measured characteristics."""
    rows = STATE.app().analytics.task_history(
        STATE.ctx(), ws(workspace), category=category, repository=repo, limit=limit
    )
    emit(
        rows,
        lambda rs: table(
            [
                (
                    r["key"],
                    r["category"],
                    r["attempts"],
                    r["commits"],
                    (r["churn_insertions"] or 0) + (r["churn_deletions"] or 0),
                    (r["final_insertions"] or 0) + (r["final_deletions"] or 0),
                    r["title"][:50],
                )
                for r in rs
            ],
            ["task", "category", "attempts", "commits", "churn", "final", "title"],
        ),
    )


@analytics_app.command("export")
@handle_errors
def export(
    target: Path,
    fmt: str = typer.Option("csv", "--format", help="csv, json or parquet"),
    dataset: str | None = typer.Option(None, help="task_facts, attempt_facts, task_commits, task_files"),
    metric: str | None = typer.Option(None),
    group_by: list[str] = typer.Option(None, "--by"),
    workspace: str | None = workspace_option(),
) -> None:
    """Export a fact table or metric result as CSV, JSON or Parquet."""
    q = AnalyticsQuery(metric=metric, group_by=group_by or []) if metric else None
    result = STATE.app().analytics.export(
        STATE.ctx(), ws(workspace), target=target, fmt=fmt, dataset=dataset, query=q
    )
    emit(result, lambda r: console.print(f"wrote {r['path']} ({r['bytes']} bytes)"))


@analytics_app.command("retrospective")
@handle_errors
def retrospective(
    period_days: int = typer.Option(14, "--days"),
    project: str | None = typer.Option(None),
    repo: str | None = typer.Option(None, "--repo"),
    no_model: bool = typer.Option(False),
    output: Path | None = typer.Option(None, "--output", "-o"),
    workspace: str | None = workspace_option(),
) -> None:
    """Generate a retrospective (deterministic statistics, optional model narrative)."""
    from workgraph.analytics.retrospective import generate_retrospective

    app = STATE.app()
    report = generate_retrospective(
        app,
        STATE.ctx(),
        ws(workspace) or app.default_workspace_id(),
        period_days=period_days,
        project=project,
        repository=repo,
        use_model=not no_model,
    )
    if output:
        output.write_text(report["content"], encoding="utf-8")
    emit(report, lambda r: console.print(r["content"], markup=False))
