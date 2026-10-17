"""Automated retrospectives.

All numbers come from the analytics service. The configured local model (if any)
only writes narrative around a JSON block of established facts; if it is unavailable
or returns invalid output, the deterministic statistical report is produced instead.
Every statement links to evidence records (task keys, attempt ids, commit SHAs).
"""

from __future__ import annotations

import json
from datetime import datetime, timedelta
from typing import TYPE_CHECKING, Any

from pydantic import BaseModel, ConfigDict, Field, ValidationError

from workgraph.analytics.service import AnalyticsFilters, AnalyticsQuery
from workgraph.core.clock import utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import WorkgraphError
from workgraph.core.events.log import record_event
from workgraph.persistence.orm import Report

if TYPE_CHECKING:
    from workgraph.app import Workgraph


class Narrative(BaseModel):
    model_config = ConfigDict(extra="forbid", title="Narrative")

    summary: str = Field(max_length=4000)
    observations: list[str] = Field(default_factory=list, max_length=12)
    opportunities: list[str] = Field(default_factory=list, max_length=8)


NARRATIVE_SYSTEM = (
    "You write concise engineering retrospectives. Use ONLY the facts in the <facts> block; do not invent numbers. "
    "Cite task keys exactly as given. Describe associations, never causation."
)


def _fmt_ms(value: Any) -> str:
    if value is None:
        return "n/a"
    seconds = float(value) / 1000
    if seconds < 120:
        return f"{seconds:.0f}s"
    if seconds < 7200:
        return f"{seconds / 60:.0f}m"
    return f"{seconds / 3600:.1f}h"


def generate_retrospective(
    app: Workgraph,
    ctx: RequestContext,
    workspace: str,
    *,
    period_days: int = 14,
    since: datetime | None = None,
    until: datetime | None = None,
    project: str | None = None,
    repository: str | None = None,
    use_model: bool = True,
) -> dict[str, Any]:
    until = until or utcnow()
    since = since or until - timedelta(days=period_days)
    analytics = app.analytics
    flt = AnalyticsFilters(since=since, until=until, project=project, repository=repository)

    def q(metric: str, group_by: list[str] | None = None, *, scoped_time: bool = True) -> list[dict[str, Any]]:
        filters = flt if scoped_time else flt.model_copy(update={"since": None, "until": None})
        return analytics.query(ctx, workspace, AnalyticsQuery(metric=metric, group_by=group_by or [], filters=filters))["rows"]

    accepted = q("tasks_accepted")
    by_category = q("tasks_accepted", ["category"])
    completed = q("tasks_completed")
    created = q("tasks_created")
    elapsed = q("task_elapsed")
    agents = q("agent_completion_rate", ["provider", "model"])
    churn = q("cumulative_churn")
    final = q("final_change_size")
    rework = q("rework_ratio")
    vpass = q("verification_pass_rate")
    failures = q("check_failures", ["check_name"])
    blocked = q("blocked_tasks", scoped_time=False)
    coupling = analytics.coupling(ctx, workspace, project=project)
    findings = analytics.failures(ctx, workspace)["findings"]
    recent = analytics.task_history(ctx, workspace, repository=repository, limit=200)
    in_period = [t for t in recent if t.get("accepted_at") and since.isoformat()[:19] <= t["accepted_at"][:19] <= until.isoformat()[:19]]
    significant = sorted(in_period, key=lambda t: -((t.get("final_insertions") or 0) + (t.get("final_deletions") or 0)))[:5]

    def first(rows: list[dict[str, Any]], key: str = "value") -> Any:
        return rows[0].get(key) if rows else None

    facts = {
        "period": {"since": since.isoformat(), "until": until.isoformat(), "project": project, "repository": repository},
        "tasks": {"created": first(created) or 0, "completed": first(completed) or 0, "accepted": first(accepted) or 0,
                  "blocked_now": first(blocked) or 0},
        "categories": {r.get("category") or "unclassified": r["value"] for r in by_category},
        "elapsed_ms": elapsed[0] if elapsed else {},
        "agents": agents,
        "code": {"cumulative_churn": churn[0] if churn else {}, "final_change": final[0] if final else {},
                 "rework_ratio": rework[0] if rework else {}},
        "verification": vpass[0] if vpass else {},
        "top_failing_checks": failures[:5],
        "coordination": coupling["summary"],
        "coupled_pairs": coupling["commonly_changed_pairs"][:5],
        "findings": [{"title": f["title"], "kind": f["kind"], "evidence": f["evidence"]} for f in findings[:10]],
        "significant_changes": [{"task": t["key"], "title": t["title"],
                                 "final_lines": (t.get("final_insertions") or 0) + (t.get("final_deletions") or 0)}
                                for t in significant],
    }
    lines = [
        f"# Retrospective: {since.date()} to {until.date()}",
        "",
        "_All numbers are measured from Workgraph records; see the evidence references in brackets._",
        "",
        "## Work completed",
        "",
        f"- Tasks created: **{facts['tasks']['created']}**, completion reported: **{facts['tasks']['completed']}**, "
        f"accepted: **{facts['tasks']['accepted']}**.",
        f"- Median time from creation to acceptance: **{_fmt_ms(facts['elapsed_ms'].get('median'))}** "
        f"(n={facts['elapsed_ms'].get('n', 0)}).",
        "",
        "## Category breakdown",
        "",
    ]
    lines += [f"- {cat}: {count}" for cat, count in facts["categories"].items()] or ["- no accepted tasks in period"]
    lines += ["", "## Bottlenecks", "", f"- Currently blocked tasks: {facts['tasks']['blocked_now']}."]
    for f in findings:
        if f["kind"] == "dependency_bottleneck":
            lines.append(f"- {f['title']} [{', '.join(f['evidence']['tasks'][:5])}]")
    lines += ["", "## Rework", ""]
    rw = facts["code"]["rework_ratio"]
    lines.append(f"- Median rework ratio (churn / final change): {rw.get('median', 'n/a') if rw else 'n/a'} "
                 f"(n={rw.get('n', 0) if rw else 0}).")
    for f in findings:
        if f["kind"] in ("high_rework", "retry_loop"):
            lines.append(f"- {f['title']} [{', '.join(f['evidence'].get('tasks', [])[:5])}]")
    lines += ["", "## Agent effectiveness", ""]
    for a in agents:
        rate = a.get("value")
        lines.append(f"- {a.get('provider')} / {a.get('model')}: {a.get('numerator')}/{a.get('denominator')} attempts "
                     f"succeeded ({rate:.0%})" if rate is not None else f"- {a.get('provider')}: no finished attempts")
    if not agents:
        lines.append("- no agent attempts finished in the period")
    lines += ["", "## Cross-repository coordination", ""]
    summary = coupling["summary"]
    lines.append(f"- Cross-repository tasks: {summary['cross_repository_tasks']} of {summary['tasks_with_code']} with code; "
                 f"average repositories per task {summary['average_repositories_per_task']}.")
    for pair in facts["coupled_pairs"]:
        lines.append(f"- {pair['pair'][0]} + {pair['pair'][1]} changed together in {pair['co_changes']} task(s).")
    lines += ["", "## Quality", ""]
    v = facts["verification"]
    if v:
        lines.append(f"- Verification pass rate: {v.get('numerator')}/{v.get('denominator')} runs passed.")
    for check in facts["top_failing_checks"]:
        lines.append(f"- Check '{check.get('check_name')}' failed {check.get('value')} time(s).")
    lines += ["", "## Significant changes", ""]
    lines += [f"- [{s['task']}] {s['title']} ({s['final_lines']} final lines)" for s in facts["significant_changes"]] or [
        "- none recorded"]
    opportunities = [f["title"] for f in findings if f["severity"] in ("warning", "error")][:5]
    lines += ["", "## Potential improvement opportunities", ""]
    lines += [f"- Investigate: {o}" for o in opportunities] or ["- No significant failure patterns detected."]
    deterministic = "\n".join(lines) + "\n"

    narrative_source = "deterministic"
    narrative_text = ""
    if use_model:
        slug = workspace
        try:
            selection = app.llm.selection("summarization", slug)
            provider = app.llm.get(selection.provider, slug)
            facts_for_model = {**facts, "highlights": lines[6:9], "summary": f"{facts['tasks']['accepted']} tasks accepted",
                               "opportunities": opportunities}
            schema = Narrative.model_json_schema()
            schema["title"] = "Narrative"
            response = provider.generate_json(selection.model, system=NARRATIVE_SYSTEM,
                                              prompt=f"<facts>{json.dumps(facts_for_model, default=str)}</facts>", schema=schema)
            narrative = Narrative.model_validate(response.data)
            narrative_text = "\n".join(["## Narrative (model-generated interpretation)", "",
                                        f"_Written by {selection.provider}/{response.model} from the measured facts above._",
                                        "", narrative.summary, ""] + [f"- {o}" for o in narrative.observations]) + "\n"
            narrative_source = f"model:{selection.provider}/{response.model}"
        except (WorkgraphError, ValidationError, OSError) as exc:
            narrative_text = f"\n_Model narrative unavailable ({type(exc).__name__}); deterministic report only._\n"
    content = deterministic + ("\n" + narrative_text if narrative_text else "")
    with app.db.session(write=True) as session:
        ws_id = app.tasks._workspace_id(session, workspace)
        report = Report(workspace_id=ws_id, kind="retrospective", title=f"Retrospective {since.date()} - {until.date()}",
                        scope=facts["period"], content_markdown=content, data=json.loads(json.dumps(facts, default=str)),
                        narrative_source=narrative_source, created_by=ctx.actor)
        session.add(report)
        session.flush()
        record_event(session, ctx, event_type="report.generated", entity_type="report", entity_id=report.id,
                     workspace_id=ws_id, data={"kind": "retrospective", "narrative_source": narrative_source})
        return {"id": report.id, "title": report.title, "narrative_source": narrative_source, "content": content,
                "facts": facts}


def list_reports(app: Workgraph, workspace: str, limit: int = 20) -> list[dict[str, Any]]:
    from sqlalchemy import select

    with app.db.session() as session:
        ws_id = app.tasks._workspace_id(session, workspace)
        rows = session.scalars(select(Report).where(Report.workspace_id == ws_id).order_by(Report.created_at.desc()).limit(limit))
        return [{"id": r.id, "kind": r.kind, "title": r.title, "narrative_source": r.narrative_source,
                 "created_at": r.created_at.isoformat(), "content": r.content_markdown} for r in rows]
