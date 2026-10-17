"""Analytics service: structured metric queries over the DuckDB warehouse."""

from __future__ import annotations

import builtins
from datetime import datetime
from pathlib import Path
from typing import Any, Literal, cast

from pydantic import BaseModel, Field, field_validator

from workgraph.analytics.metrics import BASE_DIMENSIONS, BASES, METRICS, METRICS_BY_ID, PERIODS, Metric
from workgraph.analytics.warehouse import Warehouse
from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ValidationFailed
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.security.permissions import Permission, require

FACT_TABLES = {"task_facts", "attempt_facts", "task_commits", "task_files"}
CAUSATION_NOTE = (
    "Metrics describe observed associations; differences between groups do not establish causation."
)


class AnalyticsFilters(BaseModel):
    project: str | None = None
    repository: str | None = None
    category: str | None = None
    agent: str | None = None
    provider: str | None = None
    model: str | None = None
    priority: str | None = None
    status: str | None = None
    kind: str | None = None
    since: datetime | None = None
    until: datetime | None = None


class AnalyticsQuery(BaseModel):
    metric: str
    group_by: list[str] = Field(default_factory=list)
    period: Literal["day", "week", "month", "quarter", "year"] = "week"
    filters: AnalyticsFilters = Field(default_factory=AnalyticsFilters)
    limit: int = Field(default=500, ge=1, le=5000)

    @field_validator("metric")
    @classmethod
    def _metric(cls, value: str) -> str:
        if value not in METRICS_BY_ID:
            raise ValueError(f"unknown metric {value!r}")
        return value


def metric_definition(m: Metric) -> dict[str, Any]:
    return {
        "id": m.id,
        "family": m.family,
        "title": m.title,
        "definition": m.definition,
        "unit": m.unit,
        "statistic": m.kind,
        "time_column": m.time_column,
        "dimensions": sorted(BASE_DIMENSIONS[m.base] | {"period"}),
        "caveats": m.caveats,
    }


def _jsonable(value: Any) -> Any:
    if isinstance(value, datetime):
        return value.isoformat()
    if isinstance(value, float):
        return round(value, 6)
    return value


class AnalyticsService(Service):
    def __init__(self, app: Any) -> None:
        super().__init__(app)
        self.warehouse = Warehouse(app.paths.database_file)

    def definitions(self) -> builtins.list[dict[str, Any]]:
        return [metric_definition(m) for m in METRICS]

    def _labels(self, ws_id: str) -> dict[str, dict[str, str]]:
        projects = {
            r["id"]: r["slug"]
            for r in self.warehouse.query("SELECT id, slug FROM projects WHERE workspace_id = ?", [ws_id])
        }
        repos = {
            r["id"]: r["name"]
            for r in self.warehouse.query("SELECT id, name FROM repositories WHERE workspace_id = ?", [ws_id])
        }
        return {"project": projects, "repository": repos}

    def build_sql(
        self, metric: Metric, query: AnalyticsQuery, ws_id: str, ids: dict[str, str]
    ) -> tuple[str, list[Any]]:
        allowed = BASE_DIMENSIONS[metric.base] | {"period"}
        for dim in query.group_by:
            if dim not in allowed:
                raise ValidationFailed(
                    f"metric {metric.id} cannot be grouped by {dim!r}", details={"allowed": sorted(allowed)}
                )
        if query.period not in PERIODS:
            raise ValidationFailed(f"unknown period {query.period!r}")
        base = BASES[metric.base]
        params: list[Any] = [ws_id]
        if metric.base == "tasks" and "repository" in query.group_by:
            base = f"SELECT b.*, tr.repository_id AS repository FROM ({base}) b JOIN task_repositories tr ON tr.task_id = b.id"
        where = ["ws = ?", f"({metric.where})"]
        f = query.filters
        time_col = metric.time_column
        if f.since:
            where.append(f"{time_col} >= ?")
            params.append(f.since.replace(tzinfo=None))
        if f.until:
            where.append(f"{time_col} < ?")
            params.append(f.until.replace(tzinfo=None))
        dims = BASE_DIMENSIONS[metric.base]
        simple = {
            "project": ids.get("project"),
            "category": f.category,
            "priority": f.priority,
            "status": f.status,
            "model": f.model,
            "kind": f.kind,
            "provider": f.provider,
            "agent": f.agent,
        }
        for column, value in simple.items():
            if value is None:
                continue
            if column not in dims:
                raise ValidationFailed(f"metric {metric.id} cannot be filtered by {column}")
            where.append(f"{column} = ?")
            params.append(value)
        if ids.get("repository"):
            if metric.base == "tasks" and "repository" not in query.group_by:
                where.append(
                    "EXISTS (SELECT 1 FROM task_repositories tr2 WHERE tr2.task_id = id AND tr2.repository_id = ?)"
                )
            elif "repository" in dims:
                where.append("repository = ?")
            else:
                raise ValidationFailed(f"metric {metric.id} cannot be filtered by repository")
            params.append(ids["repository"])
        select_dims = []
        for dim in query.group_by:
            select_dims.append(
                f"date_trunc('{query.period}', {time_col}) AS period" if dim == "period" else dim
            )
        group_cols = [("period" if d == "period" else d) for d in query.group_by]
        if metric.kind == "count":
            aggregates = "COUNT(*) AS value, COUNT(*) AS n"
        elif metric.kind == "sum":
            aggregates = f"SUM({metric.value}) AS value, COUNT(*) AS n"
        elif metric.kind == "ratio":
            aggregates = (
                f"{metric.numerator} AS numerator, {metric.denominator} AS denominator, "
                f"({metric.numerator})::DOUBLE / NULLIF({metric.denominator}, 0) AS value, COUNT(*) AS n"
            )
        else:
            v = metric.value
            aggregates = (
                f"COUNT({v}) AS n, AVG({v}) AS mean, quantile_cont({v}, 0.5) AS median, "
                f"quantile_cont({v}, 0.75) AS p75, quantile_cont({v}, 0.9) AS p90, "
                f"quantile_cont({v}, 0.95) AS p95, MIN({v}) AS min, MAX({v}) AS max, SUM({v}) AS sum"
            )
        sql = f"SELECT {', '.join([*select_dims, aggregates])} FROM ({base}) WHERE {' AND '.join(where)}"
        if group_cols:
            sql += f" GROUP BY {', '.join(group_cols)} ORDER BY {', '.join(group_cols)}"
        sql += f" LIMIT {int(query.limit)}"
        return sql, params

    def query(self, ctx: RequestContext, workspace: str | None, query: AnalyticsQuery) -> dict[str, Any]:
        require(ctx, Permission.ANALYTICS_READ)
        metric = METRICS_BY_ID[query.metric]
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            ids: dict[str, str] = {}
            if query.filters.project:
                ids["project"] = resolve.project(session, ws_id, query.filters.project).id
            if query.filters.repository:
                ids["repository"] = resolve.repository(
                    session, ws_id, query.filters.repository, include_archived=True
                ).id
        sql, params = self.build_sql(metric, query, ws_id, ids)
        rows = self.warehouse.query(sql, params)
        labels = self._labels(ws_id)
        for row in rows:
            for dim in ("project", "repository"):
                if dim in row and row[dim] is not None:
                    row[f"{dim}_label"] = labels[dim].get(row[dim], row[dim])
            for key, value in list(row.items()):
                row[key] = _jsonable(value)
        return {
            "metric": metric_definition(metric),
            "query": query.model_dump(mode="json"),
            "rows": rows,
            "sample_size": sum(int(r.get("n") or 0) for r in rows),
            "provenance": {
                "sql": sql,
                "source": "DuckDB snapshot of the SQLite database",
                "generated_at": utcnow().isoformat(),
            },
            "notes": [*metric.caveats, CAUSATION_NOTE],
        }

    def compare(
        self,
        ctx: RequestContext,
        workspace: str | None,
        metric: str,
        *,
        a: tuple[datetime, datetime],
        b: tuple[datetime, datetime],
        group_by: builtins.list[str] | None = None,
    ) -> dict[str, Any]:
        """Same metric over two periods, with absolute and relative change per group."""
        group = [g for g in (group_by or []) if g != "period"]
        first = self.query(
            ctx,
            workspace,
            AnalyticsQuery(metric=metric, group_by=group, filters=AnalyticsFilters(since=a[0], until=a[1])),
        )
        second = self.query(
            ctx,
            workspace,
            AnalyticsQuery(metric=metric, group_by=group, filters=AnalyticsFilters(since=b[0], until=b[1])),
        )
        key_field = "median" if METRICS_BY_ID[metric].kind == "distribution" else "value"

        def key(row: dict[str, Any]) -> tuple[Any, ...]:
            return tuple(row.get(g) for g in group)

        a_rows = {key(r): r for r in first["rows"]}
        b_rows = {key(r): r for r in second["rows"]}
        out = []
        for k in sorted(set(a_rows) | set(b_rows), key=str):
            va = (a_rows.get(k) or {}).get(key_field)
            vb = (b_rows.get(k) or {}).get(key_field)
            delta = (vb - va) if isinstance(va, int | float) and isinstance(vb, int | float) else None
            out.append(
                {
                    **dict(zip(group, k, strict=True)),
                    "a": va,
                    "b": vb,
                    "delta": delta,
                    "relative_change": (delta / va) if delta is not None and va else None,
                    "n_a": (a_rows.get(k) or {}).get("n", 0),
                    "n_b": (b_rows.get(k) or {}).get("n", 0),
                }
            )
        return {
            "metric": first["metric"],
            "statistic": key_field,
            "period_a": [x.isoformat() for x in a],
            "period_b": [x.isoformat() for x in b],
            "rows": out,
            "notes": [CAUSATION_NOTE],
        }

    def overview(self, ctx: RequestContext, workspace: str | None, *, period: str = "week") -> dict[str, Any]:
        """Bundle of common dashboard metrics (all from persisted data)."""

        def q(metric: str, group_by: builtins.list[str] | None = None) -> builtins.list[dict[str, Any]]:
            return self.query(
                ctx,
                workspace,
                AnalyticsQuery(metric=metric, group_by=group_by or [], period=cast(Any, period)),
            )["rows"]

        return {
            "created_by_period": q("tasks_created", ["period"]),
            "accepted_by_period": q("tasks_accepted", ["period"]),
            "completed_by_period": q("tasks_completed", ["period"]),
            "open_by_status": q("open_backlog", ["status"]),
            "by_category": q("tasks_created", ["category"]),
            "execution_duration": q("execution_duration"),
            "queue_wait": q("queue_wait"),
            "task_elapsed": q("task_elapsed"),
            "first_pass_success": q("first_pass_success"),
            "agents": q("agent_completion_rate", ["provider", "model"]),
            "agent_duration": q("agent_duration", ["provider", "model"]),
            "churn": q("cumulative_churn"),
            "final_change": q("final_change_size"),
            "rework": q("rework_ratio"),
            "check_failures_by_period": q("check_failures", ["period", "kind"]),
            "verification_pass_rate": q("verification_pass_rate", ["period"]),
            "repository_volume": q("repository_change_volume", ["repository"]),
            "module_hotspots": q("module_hotspots", ["module"])[:15],
        }

    def export(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        target: Path,
        fmt: str,
        dataset: str | None = None,
        query: AnalyticsQuery | None = None,
    ) -> dict[str, Any]:
        require(ctx, Permission.ANALYTICS_READ)
        if fmt not in ("csv", "json", "parquet"):
            raise ValidationFailed("format must be csv, json or parquet")
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
        if query is not None:
            metric = METRICS_BY_ID[query.metric]
            ids: dict[str, str] = {}
            with self.db.session() as session:
                if query.filters.project:
                    ids["project"] = resolve.project(session, ws_id, query.filters.project).id
                if query.filters.repository:
                    ids["repository"] = resolve.repository(
                        session, ws_id, query.filters.repository, include_archived=True
                    ).id
            sql, params = self.build_sql(metric, query, ws_id, ids)
        elif dataset in FACT_TABLES:
            if dataset in ("task_facts", "attempt_facts"):
                sql, params = f"SELECT * FROM {dataset} WHERE workspace_id = ?", [ws_id]
            else:
                sql = f"SELECT d.* FROM {dataset} d JOIN tasks t ON t.id = d.task_id WHERE t.workspace_id = ?"
                params = [ws_id]
        else:
            raise ValidationFailed(f"dataset must be one of {sorted(FACT_TABLES)} or a metric query")
        path = self.warehouse.export(sql, params, target, fmt)
        return {
            "path": str(path),
            "format": fmt,
            "dataset": dataset or query.metric if query else dataset,
            "bytes": path.stat().st_size,
        }

    # ------------------------------------------------------------------ higher-level analyses

    def coupling(self, ctx: RequestContext, workspace: str | None, **kwargs: Any) -> dict[str, Any]:
        require(ctx, Permission.ANALYTICS_READ)
        from workgraph.analytics.coupling import coupling_report

        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
        return coupling_report(self, ws_id, **kwargs)

    def estimate(self, ctx: RequestContext, workspace: str | None, **kwargs: Any) -> dict[str, Any]:
        require(ctx, Permission.ANALYTICS_READ)
        from workgraph.analytics.estimation import estimate

        return estimate(self.app, ctx, workspace, **kwargs)

    def failures(self, ctx: RequestContext, workspace: str | None) -> dict[str, Any]:
        require(ctx, Permission.ANALYTICS_READ)
        from workgraph.analytics.failures import detect_failures

        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
        return detect_failures(self, ws_id)

    def task_history(
        self,
        ctx: RequestContext,
        workspace: str | None,
        *,
        category: str | None = None,
        repository: str | None = None,
        limit: int = 50,
    ) -> builtins.list[dict[str, Any]]:
        """Historical (accepted) task results with their measured characteristics."""
        require(ctx, Permission.ANALYTICS_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            repo_id = (
                resolve.repository(session, ws_id, repository, include_archived=True).id
                if repository
                else None
            )
        sql = (
            "SELECT key, title, category, status, attempts, retries, execution_ms, queue_wait_ms, elapsed_ms, "
            "commits, churn_insertions, churn_deletions, final_insertions, final_deletions, files_touched, "
            "repositories_touched, reopen_count, accepted_at FROM task_facts WHERE workspace_id = ? "
            "AND accepted_at IS NOT NULL"
        )
        params: list[Any] = [ws_id]
        if category:
            sql += " AND category = ?"
            params.append(category)
        if repo_id:
            sql += " AND EXISTS (SELECT 1 FROM task_repositories tr WHERE tr.task_id = id AND tr.repository_id = ?)"
            params.append(repo_id)
        sql += f" ORDER BY accepted_at DESC LIMIT {int(limit)}"
        return [{k: _jsonable(v) for k, v in row.items()} for row in self.warehouse.query(sql, params)]
