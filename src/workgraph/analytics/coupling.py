"""Cross-repository coupling: observed co-change versus declared dependencies.

A repository is "touched" by a task when the task has at least one linked commit
(confidence >= 0.5, distinct by patch id) in it. Two repositories co-change when the
same task touches both. Every edge lists the supporting tasks and commits, so each
number is traceable. Findings are evidence-backed observations, not architectural
verdicts.
"""

from __future__ import annotations

from collections import defaultdict
from itertools import combinations
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from workgraph.analytics.service import AnalyticsService


def coupling_report(service: AnalyticsService, ws_id: str, *, min_support: int = 1, project: str | None = None) -> dict[str, Any]:
    wh = service.warehouse
    repos = {r["id"]: r["name"] for r in wh.query("SELECT id, name FROM repositories WHERE workspace_id = ?", [ws_id])}
    project_filter = ""
    params: list[Any] = [ws_id]
    if project:
        project_filter = " AND EXISTS (SELECT 1 FROM task_projects tp JOIN projects p ON p.id = tp.project_id " \
                         "WHERE tp.task_id = t.id AND (p.slug = ? OR p.id = ?))"
        params += [project, project]
    rows = wh.query(
        "SELECT t.id AS task_id, t.key, t.elapsed_ms, t.cycle_ms, t.category, tc.repository_id, tc.sha "
        "FROM task_commits tc JOIN task_facts t ON t.id = tc.task_id "
        f"WHERE t.workspace_id = ? AND NOT tc.is_merge{project_filter}", params)
    task_repos: dict[str, set[str]] = defaultdict(set)
    task_meta: dict[str, dict[str, Any]] = {}
    commits_by_task_repo: dict[tuple[str, str], list[str]] = defaultdict(list)
    for row in rows:
        task_repos[row["task_id"]].add(row["repository_id"])
        task_meta[row["task_id"]] = {"key": row["key"], "elapsed_ms": row["elapsed_ms"], "cycle_ms": row["cycle_ms"],
                                     "category": row["category"]}
        commits_by_task_repo[(row["task_id"], row["repository_id"])].append(row["sha"])
    pair_tasks: dict[tuple[str, str], list[str]] = defaultdict(list)
    touch_counts: dict[str, int] = defaultdict(int)
    for task_id, touched in task_repos.items():
        for repo in touched:
            touch_counts[repo] += 1
        for a, b in combinations(sorted(touched), 2):
            pair_tasks[(a, b)].append(task_id)
    declared = {(d["from_repository_id"], d["to_repository_id"]) for d in wh.query(
        "SELECT from_repository_id, to_repository_id FROM repository_dependencies")}
    declared_undirected = {tuple(sorted(p)) for p in declared}
    edges = []
    for (a, b), tasks in sorted(pair_tasks.items(), key=lambda kv: -len(kv[1])):
        if len(tasks) < min_support:
            continue
        union = touch_counts[a] + touch_counts[b] - len(tasks)
        edges.append({
            "source": a, "target": b, "source_name": repos.get(a, a), "target_name": repos.get(b, b),
            "co_changes": len(tasks),
            "jaccard": round(len(tasks) / union, 4) if union else None,
            "declared": (a, b) in declared_undirected,
            "relationship": "declared+observed" if (a, b) in declared_undirected else "observed",
            "evidence": [{"task_id": t, "task_key": task_meta[t]["key"],
                          "commits": {repos.get(a, a): commits_by_task_repo[(t, a)][:10],
                                      repos.get(b, b): commits_by_task_repo[(t, b)][:10]}} for t in tasks[:50]],
        })
    observed_pairs = {(e["source"], e["target"]) for e in edges}
    declared_only = [{"source_name": repos.get(a, a), "target_name": repos.get(b, b), "relationship": "declared_only"}
                     for a, b in declared_undirected if (a, b) not in observed_pairs and a in repos and b in repos]
    matrix = {repos.get(a, a): {repos.get(b, b): 0 for b in repos} for a in repos}
    for (a, b), tasks in pair_tasks.items():
        if a in repos and b in repos:
            matrix[repos[a]][repos[b]] = matrix[repos[b]][repos[a]] = len(tasks)
    for repo, count in touch_counts.items():
        if repo in repos:
            matrix[repos[repo]][repos[repo]] = count
    cross = [t for t, r in task_repos.items() if len(r) > 1]
    single = [t for t, r in task_repos.items() if len(r) == 1]

    def median(values: list[float]) -> float | None:
        clean = sorted(v for v in values if v is not None)
        if not clean:
            return None
        mid = len(clean) // 2
        return clean[mid] if len(clean) % 2 else (clean[mid - 1] + clean[mid]) / 2

    integration = wh.query(
        "SELECT r.task_id, COUNT(*) AS checks, COUNT(*) FILTER (WHERE c.status IN ('failed','error')) AS failed "
        "FROM verification_check_results c JOIN verification_runs r ON r.id = c.run_id "
        "WHERE r.workspace_id = ? AND c.kind = 'integration' GROUP BY r.task_id", [ws_id])
    integ_checks = sum(int(r["checks"]) for r in integration)
    integ_failed = sum(int(r["failed"]) for r in integration)
    module_rows = wh.query(
        "SELECT f.task_id, f.repository_id, f.module FROM task_files f JOIN tasks t ON t.id = f.task_id "
        "WHERE t.workspace_id = ?", [ws_id])
    modules_by_task: dict[str, set[tuple[str, str]]] = defaultdict(set)
    for r in module_rows:
        modules_by_task[r["task_id"]].add((r["repository_id"], r["module"]))
    module_impact: dict[str, dict[str, Any]] = {}
    for task_id in cross:
        for repo_id, module in modules_by_task.get(task_id, set()):
            key = f"{repos.get(repo_id, repo_id)}:{module}"
            entry = module_impact.setdefault(key, {"module": key, "cross_repo_tasks": 0, "tasks": []})
            entry["cross_repo_tasks"] += 1
            entry["tasks"].append(task_meta[task_id]["key"])
    candidates = [
        {**e, "observation": f"{e['source_name']} and {e['target_name']} changed together in {e['co_changes']} task(s)"
         + (" without a declared dependency" if not e["declared"] else "")}
        for e in edges if e["co_changes"] >= max(2, min_support) and not e["declared"]
    ]
    return {
        "repositories": [{"id": rid, "name": name, "tasks_touching": touch_counts.get(rid, 0)} for rid, name in repos.items()],
        "edges": edges,
        "declared_only": declared_only,
        "matrix": matrix,
        "summary": {
            "tasks_with_code": len(task_repos),
            "cross_repository_tasks": len(cross),
            "average_repositories_per_task": round(sum(len(r) for r in task_repos.values()) / len(task_repos), 3)
            if task_repos else None,
            "median_elapsed_ms_cross_repo": median([task_meta[t]["elapsed_ms"] for t in cross]),
            "median_elapsed_ms_single_repo": median([task_meta[t]["elapsed_ms"] for t in single]),
            "integration_checks": integ_checks,
            "integration_failure_rate": round(integ_failed / integ_checks, 4) if integ_checks else None,
        },
        "commonly_changed_pairs": [{"pair": [e["source_name"], e["target_name"]], "co_changes": e["co_changes"]}
                                   for e in edges[:10]],
        "modules_with_cross_repo_impact": sorted(module_impact.values(), key=lambda m: -m["cross_repo_tasks"])[:20],
        "coupling_candidates": candidates,
        "notes": [
            "Observed co-change indicates repositories modified within the same task; it is evidence of coupling, "
            "not proof of an architectural dependency.",
            "Coordination durations compare medians of different task populations and do not imply causation.",
        ],
    }
