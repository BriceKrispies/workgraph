"""Historical estimation from comparable completed work.

Comparable tasks are accepted tasks scored by: same classification, semantic or
lexical text similarity, repository overlap and file/module overlap. Distributions
are reported with sample sizes and percentiles. With fewer than ``MIN_SAMPLE``
comparable tasks an explicit ``insufficient_data`` result is returned - no number is
fabricated.
"""

from __future__ import annotations

from collections import defaultdict
from typing import TYPE_CHECKING, Any

from sqlalchemy import select

from workgraph.core.context import RequestContext
from workgraph.core.errors import WorkgraphError
from workgraph.core.services import resolve
from workgraph.persistence.orm import Task, TaskRepository

if TYPE_CHECKING:
    from workgraph.app import Workgraph

MIN_SAMPLE = 3
TOP_K = 12
MIN_SCORE = 0.15


def percentile(values: list[float], q: float) -> float | None:
    data = sorted(values)
    if not data:
        return None
    pos = (len(data) - 1) * q
    lo, hi = int(pos), min(int(pos) + 1, len(data) - 1)
    return data[lo] + (data[hi] - data[lo]) * (pos - lo)


def distribution(values: list[Any]) -> dict[str, Any]:
    clean = [float(v) for v in values if v is not None]
    return {
        "n": len(clean),
        "p25": percentile(clean, 0.25),
        "median": percentile(clean, 0.5),
        "p75": percentile(clean, 0.75),
        "p90": percentile(clean, 0.9),
        "min": min(clean) if clean else None,
        "max": max(clean) if clean else None,
    }


def estimate(
    app: Workgraph,
    ctx: RequestContext,
    workspace: str | None,
    *,
    task: str | None = None,
    title: str | None = None,
    description: str = "",
    category: str | None = None,
    repositories: list[str] | None = None,
) -> dict[str, Any]:
    with app.db.session() as session:
        ws_id = app.tasks._workspace_id(session, workspace)
        target_id = None
        repo_ids: set[str] = set()
        if task:
            row = resolve.task(session, ws_id, task)
            target_id, title, description = row.id, row.title, row.description
            category = category or row.category
            repo_ids = set(
                session.scalars(select(TaskRepository.repository_id).where(TaskRepository.task_id == row.id))
            )
        for ref in repositories or []:
            repo_ids.add(resolve.repository(session, ws_id, ref, include_archived=True).id)
        accepted = {
            t.id: t
            for t in session.scalars(
                select(Task).where(
                    Task.workspace_id == ws_id, Task.accepted_at.is_not(None), Task.id != (target_id or "")
                )
            )
        }
        repos_of: dict[str, set[str]] = defaultdict(set)
        for tid, rid in session.execute(
            select(TaskRepository.task_id, TaskRepository.repository_id).where(
                TaskRepository.task_id.in_(list(accepted))
            )
        ):
            repos_of[tid].add(rid)
    if not title:
        raise WorkgraphError("provide a task or a title to estimate")
    text_scores: dict[str, float] = {}
    method = "lexical"
    if target_id:
        try:
            similar = app.search.similar(ctx, workspace, target_id, limit=50, completed_only=True)
            method = similar["method"]
            text_scores = {s["task_id"]: float(s["similarity"]) for s in similar["similar"]}
        except WorkgraphError:
            text_scores = {}
    if not text_scores:
        hits = app.search.lexical(ctx, workspace, f"{title} {description[:300]}", limit=50, match_any=True)
        text_scores = {h.task_id: 1.0 / (rank + 1) for rank, h in enumerate(hits)}
    my_modules: set[str] = set()
    if target_id:
        my_modules = {
            r["module"]
            for r in app.analytics.warehouse.query(
                "SELECT DISTINCT module FROM task_files WHERE task_id = ?", [target_id]
            )
        }
    scored = []
    for tid, other in accepted.items():
        reasons: list[str] = []
        score = 0.0
        if category and other.category == category:
            score += 0.35
            reasons.append(f"same category ({category})")
        text = text_scores.get(tid, 0.0)
        if text:
            score += 0.4 * min(1.0, text)
            reasons.append(f"{method} text similarity {text:.2f}")
        if repo_ids and repos_of.get(tid):
            overlap = len(repo_ids & repos_of[tid]) / len(repo_ids | repos_of[tid])
            if overlap:
                score += 0.2 * overlap
                reasons.append(f"repository overlap {overlap:.2f}")
        if my_modules:
            theirs = {
                r["module"]
                for r in app.analytics.warehouse.query(
                    "SELECT DISTINCT module FROM task_files WHERE task_id = ?", [tid]
                )
            }
            if theirs & my_modules:
                score += 0.05 * len(theirs & my_modules) / len(theirs | my_modules)
                reasons.append("touches the same modules")
        if score >= MIN_SCORE:
            scored.append((score, tid, other.key, other.title, reasons))
    scored.sort(reverse=True)
    chosen = scored[:TOP_K]
    basis = [
        {"task_id": tid, "key": key, "title": t, "score": round(s, 3), "reasons": r}
        for s, tid, key, t, r in chosen
    ]
    if len(chosen) < MIN_SAMPLE:
        return {
            "status": "insufficient_data",
            "message": f"only {len(chosen)} comparable accepted task(s) found; at least {MIN_SAMPLE} are required",
            "comparable_tasks": basis,
            "min_sample": MIN_SAMPLE,
        }
    ids = [tid for _, tid, *_ in chosen]
    placeholders = ",".join("?" for _ in ids)
    facts = {
        r["id"]: r
        for r in app.analytics.warehouse.query(f"SELECT * FROM task_facts WHERE id IN ({placeholders})", ids)
    }

    def collect(field: str) -> list[Any]:
        return [facts[i].get(field) for i in ids if i in facts]

    churn = [
        (f.get("churn_insertions") or 0) + (f.get("churn_deletions") or 0)
        for f in facts.values()
        if f.get("commits")
    ]
    final = [
        (f.get("final_insertions") or 0) + (f.get("final_deletions") or 0)
        for f in facts.values()
        if f.get("final_insertions") is not None
    ]
    runs = app.analytics.warehouse.query(
        f"SELECT status FROM verification_runs WHERE task_id IN ({placeholders})", ids
    )
    passed = sum(1 for r in runs if r["status"] == "passed")
    agents = app.analytics.warehouse.query(
        f"SELECT provider, COALESCE(model, '(unspecified)') AS model, COUNT(*) AS attempts, "
        f"COUNT(*) FILTER (WHERE status = 'succeeded') AS succeeded, quantile_cont(duration_ms, 0.5) AS median_ms "
        f"FROM agent_attempts WHERE parent_attempt_id IS NULL AND task_id IN ({placeholders}) GROUP BY provider, model",
        ids,
    )
    recommendations = sorted(
        [
            {
                "provider": a["provider"],
                "model": a["model"],
                "attempts": a["attempts"],
                "succeeded": a["succeeded"],
                "success_rate": round(a["succeeded"] / a["attempts"], 3) if a["attempts"] else None,
                "median_duration_ms": a["median_ms"],
            }
            for a in agents
        ],
        key=lambda a: (-float(a["success_rate"] or 0), -int(a["attempts"] or 0)),
    )
    return {
        "status": "ok",
        "basis": basis,
        "sample_size": len(ids),
        "method": method,
        "distributions": {
            "execution_ms": distribution(collect("execution_ms")),
            "queue_ms": distribution(collect("queue_wait_ms")),
            "elapsed_ms": distribution(collect("elapsed_ms")),
            "attempts": distribution(collect("attempts")),
            "files_changed": distribution(collect("files_touched")),
            "cumulative_churn": distribution(churn),
            "final_change_size": distribution(final),
        },
        "verification": {
            "runs": len(runs),
            "passed": passed,
            "pass_rate": round(passed / len(runs), 3) if runs else None,
        },
        "agent_recommendations": recommendations,
        "uncertainty": "Ranges are the 25th-75th/90th percentiles of comparable tasks; small samples are volatile.",
        "caveats": [
            "Agent recommendations reflect which agents happened to be used on similar tasks (selection bias); "
            "they are evidence, not a controlled comparison.",
        ],
    }
