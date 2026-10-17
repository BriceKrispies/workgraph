"""Deterministic failure-pattern detection with linked evidence."""

from __future__ import annotations

import hashlib
import json
from collections import defaultdict
from typing import TYPE_CHECKING, Any

from sqlalchemy import select

from workgraph.core.clock import utcnow
from workgraph.persistence.orm import Finding

if TYPE_CHECKING:
    from workgraph.analytics.service import AnalyticsService
    from workgraph.app import Workgraph


def _finding(
    kind: str, title: str, severity: str, value: float | None, n: int, evidence: dict[str, Any]
) -> dict[str, Any]:
    fingerprint = hashlib.sha256(f"{kind}|{title}".encode()).hexdigest()[:32]
    return {
        "kind": kind,
        "title": title,
        "severity": severity,
        "metric_value": value,
        "sample_size": n,
        "evidence": evidence,
        "fingerprint": fingerprint,
    }


def detect_failures(service: AnalyticsService, ws_id: str) -> dict[str, Any]:
    wh = service.warehouse
    findings: list[dict[str, Any]] = []

    # 1. Frequently failing tests (parsed failure signatures across runs).
    rows = wh.query(
        "SELECT c.failure_signatures, c.name, r.task_id, r.attempt_id, t.key FROM verification_check_results c "
        "JOIN verification_runs r ON r.id = c.run_id JOIN tasks t ON t.id = r.task_id "
        "WHERE r.workspace_id = ? AND c.status IN ('failed','error')",
        [ws_id],
    )
    by_sig: dict[str, list[dict[str, Any]]] = defaultdict(list)
    by_check: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for row in rows:
        by_check[row["name"]].append(row)
        for sig in json.loads(row["failure_signatures"] or "[]"):
            by_sig[sig].append(row)
    for sig, items in by_sig.items():
        if len(items) >= 2 and sig.startswith("test:"):
            findings.append(
                _finding(
                    "frequently_failing_test",
                    f"{sig[5:]} failed {len(items)} times",
                    "warning" if len(items) < 5 else "error",
                    len(items),
                    len(items),
                    {
                        "tasks": sorted({i["key"] for i in items}),
                        "attempts": sorted({i["attempt_id"] for i in items if i["attempt_id"]}),
                    },
                )
            )
    # 2. Common failure categories (attempt exit reasons, failing check names).
    exits = wh.query(
        "SELECT exit_reason, COUNT(*) AS n, list(task_key) AS tasks, list(id) AS attempts FROM attempt_facts "
        "WHERE workspace_id = ? AND status IN ('failed','timed_out') GROUP BY exit_reason ORDER BY n DESC",
        [ws_id],
    )
    for row in exits:
        if row["n"] >= 2:
            findings.append(
                _finding(
                    "failure_category",
                    f"{row['n']} attempts failed with '{row['exit_reason']}'",
                    "info",
                    row["n"],
                    row["n"],
                    {"tasks": sorted(set(row["tasks"]))[:50], "attempts": row["attempts"][:50]},
                )
            )
    for name, items in by_check.items():
        if len(items) >= 3:
            findings.append(
                _finding(
                    "failing_check",
                    f"check '{name}' failed {len(items)} times",
                    "warning",
                    len(items),
                    len(items),
                    {"tasks": sorted({i["key"] for i in items})[:50]},
                )
            )
    # 3. Repeated agent retry loops.
    loops = wh.query(
        "SELECT key, id, attempts, failed_attempts FROM task_facts WHERE workspace_id = ? AND attempts >= 3 "
        "ORDER BY attempts DESC",
        [ws_id],
    )
    for row in loops:
        attempt_ids = [
            a["id"]
            for a in wh.query("SELECT id FROM agent_attempts WHERE task_id = ? ORDER BY number", [row["id"]])
        ]
        findings.append(
            _finding(
                "retry_loop",
                f"{row['key']} needed {row['attempts']} attempts",
                "warning",
                row["attempts"],
                row["attempts"],
                {"tasks": [row["key"]], "attempts": attempt_ids},
            )
        )
    # 4. Subsystems with high rework (modules edited repeatedly within tasks).
    rework = wh.query(
        "SELECT f.module, r.name AS repository, SUM(f.edits) AS edits, COUNT(DISTINCT f.task_id) AS tasks, "
        "list(DISTINCT t.key) AS keys FROM task_files f JOIN tasks t ON t.id = f.task_id "
        "JOIN repositories r ON r.id = f.repository_id WHERE t.workspace_id = ? GROUP BY f.module, r.name "
        "HAVING SUM(f.edits) > COUNT(DISTINCT f.task_id) * 2 ORDER BY edits DESC LIMIT 10",
        [ws_id],
    )
    for row in rework:
        findings.append(
            _finding(
                "high_rework",
                f"{row['repository']}:{row['module']} edited {row['edits']} times across "
                f"{row['tasks']} task(s)",
                "info",
                row["edits"],
                row["tasks"],
                {"tasks": row["keys"][:50]},
            )
        )
    # 5. Repositories with high integration failure rates.
    integ = wh.query(
        "SELECT r2.name AS repository, COUNT(*) AS checks, COUNT(*) FILTER (WHERE c.status IN ('failed','error')) AS failed, "
        "list(DISTINCT t.key) AS keys FROM verification_check_results c JOIN verification_runs r ON r.id = c.run_id "
        "JOIN tasks t ON t.id = r.task_id JOIN task_repositories tr ON tr.task_id = t.id "
        "JOIN repositories r2 ON r2.id = tr.repository_id WHERE r.workspace_id = ? AND c.kind = 'integration' "
        "GROUP BY r2.name",
        [ws_id],
    )
    for row in integ:
        if row["checks"] >= 2 and row["failed"] / row["checks"] >= 0.3:
            findings.append(
                _finding(
                    "integration_failures",
                    f"{row['repository']}: {row['failed']}/{row['checks']} integration checks failed",
                    "warning",
                    row["failed"] / row["checks"],
                    row["checks"],
                    {"tasks": row["keys"][:50]},
                )
            )
    # 6. Tasks frequently blocked by the same dependency.
    blockers = wh.query(
        "SELECT b.key AS blocker, COUNT(*) AS blocked, list(t.key) AS keys FROM task_dependencies d "
        "JOIN tasks t ON t.id = d.task_id JOIN tasks b ON b.id = d.depends_on_id "
        "WHERE t.workspace_id = ? GROUP BY b.key HAVING COUNT(*) >= 2 ORDER BY blocked DESC",
        [ws_id],
    )
    for row in blockers:
        findings.append(
            _finding(
                "dependency_bottleneck",
                f"{row['blocker']} blocks {row['blocked']} tasks",
                "info",
                row["blocked"],
                row["blocked"],
                {"blocker": row["blocker"], "tasks": row["keys"][:50]},
            )
        )
    # 7. Changes repeatedly followed by regression fixes.
    regress = wh.query(
        "SELECT o.key AS original, COUNT(*) AS n, list(s.key) AS fixes FROM task_relations rel "
        "JOIN tasks s ON s.id = rel.source_task_id JOIN tasks o ON o.id = rel.target_task_id "
        "WHERE s.workspace_id = ? AND rel.relation_type IN ('regression_of','follows_up') GROUP BY o.key",
        [ws_id],
    )
    for row in regress:
        commits = [
            r["sha"]
            for r in wh.query(
                "SELECT tc.sha FROM task_commits tc JOIN tasks t ON t.id = tc.task_id WHERE t.key = ? AND t.workspace_id = ?",
                [row["original"], ws_id],
            )
        ]
        findings.append(
            _finding(
                "regression_follow_up",
                f"{row['original']} was followed by {row['n']} fix task(s)",
                "warning" if row["n"] > 1 else "info",
                row["n"],
                row["n"],
                {"original": row["original"], "fixes": row["fixes"], "commits": commits[:20]},
            )
        )
    return {
        "generated_at": utcnow().isoformat(),
        "findings": findings,
        "note": "Findings are computed deterministically from recorded tasks, attempts, checks and commits.",
    }


def persist_findings(app: Workgraph, workspace: str) -> dict[str, Any]:
    from workgraph.core.context import RequestContext

    ctx = RequestContext.system("failure-analysis")
    report = app.analytics.failures(ctx, workspace)
    with app.db.session(write=True) as session:
        ws_id = app.tasks._workspace_id(session, workspace)
        seen = 0
        for f in report["findings"]:
            row = session.scalar(
                select(Finding).where(Finding.workspace_id == ws_id, Finding.fingerprint == f["fingerprint"])
            )
            if row is None:
                session.add(
                    Finding(
                        workspace_id=ws_id,
                        kind=f["kind"],
                        fingerprint=f["fingerprint"],
                        title=f["title"],
                        severity=f["severity"],
                        metric_value=f["metric_value"],
                        sample_size=f["sample_size"],
                        evidence=f["evidence"],
                    )
                )
            else:
                row.title, row.severity, row.metric_value = f["title"], f["severity"], f["metric_value"]
                row.sample_size, row.evidence, row.last_detected_at = (
                    f["sample_size"],
                    f["evidence"],
                    utcnow(),
                )
            seen += 1
    return {"findings": seen}
