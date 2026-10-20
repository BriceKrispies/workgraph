"""Job kind -> handler mapping. Handlers run outside database transactions."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from workgraph.core.context import RequestContext
from workgraph.verification.configs import parse_tier

if TYPE_CHECKING:
    from workgraph.app import Workgraph
    from workgraph.jobs.worker import Handler, JobContext


def _ctx(job_ctx: JobContext, component: str) -> RequestContext:
    ctx = RequestContext.system(component)
    if job_ctx.job.correlation_id:
        from dataclasses import replace

        ctx = replace(ctx, correlation_id=job_ctx.job.correlation_id)
    return ctx.caused_by(job_ctx.job.causation_id)


def git_sync(app: Workgraph, job: JobContext) -> dict[str, Any]:
    result = app.git.sync(
        _ctx(job, "git"), job.job.payload["repository_id"], mode=job.job.payload.get("mode", "incremental")
    )
    return result.model_dump()


def tracker_sync(app: Workgraph, job: JobContext) -> dict[str, Any]:
    payload = job.job.payload
    report = app.trackers.sync(
        _ctx(job, "tracker-sync"), payload["connection"], full=bool(payload.get("full"))
    )
    return report.as_dict()


def analyze_commits(app: Workgraph, job: JobContext) -> dict[str, Any]:
    analyzed = 0
    for commit_id in job.job.payload["commit_ids"]:
        job.check_cancelled()
        if app.git.analyze_commit(commit_id):
            analyzed += 1
    return {"analyzed": analyzed}


def compute_change_surface(app: Workgraph, job: JobContext) -> dict[str, Any]:
    surfaces = app.git.compute_change_surface(_ctx(job, "git"), job.job.payload["task_id"])
    return {"repositories": len(surfaces)}


def classify_task(app: Workgraph, job: JobContext) -> dict[str, Any]:
    out = app.classification.run_classification(
        _ctx(job, "classifier"),
        job.job.payload["task_id"],
        provider=job.job.payload.get("provider"),
        model=job.job.payload.get("model"),
        promote=job.job.payload.get("promote"),
        batch_id=job.job.payload.get("batch_id"),
    )
    return {"classification_id": out.id, "status": out.status, "category": out.category}


def embed_task(app: Workgraph, job: JobContext) -> dict[str, Any]:
    return app.search.embed_task(
        job.job.payload["task_id"],
        provider=job.job.payload.get("provider"),
        model=job.job.payload.get("model"),
    )


def evaluate_classifier(app: Workgraph, job: JobContext) -> dict[str, Any]:
    payload = job.job.payload
    out = app.classification.evaluate(
        _ctx(job, "classifier"),
        payload["workspace"],
        provider=payload.get("provider"),
        model=payload.get("model"),
    )
    return {"evaluation_id": out["id"], "accuracy": out["accuracy"]}


def agent_run(app: Workgraph, job: JobContext) -> dict[str, Any]:
    return app.runs.execute(job)


def verify(app: Workgraph, job: JobContext) -> dict[str, Any]:
    payload = job.job.payload
    run = app.verification.run(
        _ctx(job, "verifier"),
        None,
        payload["task_id"],
        attempt_id=payload.get("attempt_id"),
        should_cancel=job.cancel_requested,
        tier=parse_tier(payload.get("tier")),
    )
    return {"run_id": run.id, "status": run.status}


def stack_integrate(app: Workgraph, job: JobContext) -> dict[str, Any]:
    payload = job.job.payload
    return app.stack_integration.integrate(_ctx(job, "stacks"), payload["stack_id"], payload["repository_id"])


def stack_verify(app: Workgraph, job: JobContext) -> dict[str, Any]:
    return app.stack_verification.verify(
        _ctx(job, "stacks"), job.job.payload["stack_id"], should_cancel=job.cancel_requested
    )


def decompose_task(app: Workgraph, job: JobContext) -> dict[str, Any]:
    return app.scheduler.decompose(
        _ctx(job, "scheduler"), job.job.payload["task_id"], create=job.job.payload.get("create", True)
    )


def automation_action(app: Workgraph, job: JobContext) -> dict[str, Any]:
    return app.automation.execute_run(job.job.payload["run_id"])


def generate_report(app: Workgraph, job: JobContext) -> dict[str, Any]:
    from workgraph.analytics.retrospective import generate_retrospective

    payload = job.job.payload
    report = generate_retrospective(
        app, _ctx(job, "reports"), payload["workspace"], **payload.get("scope", {})
    )
    return {"report_id": report["id"], "narrative_source": report["narrative_source"]}


def failure_analysis(app: Workgraph, job: JobContext) -> dict[str, Any]:
    from workgraph.analytics.failures import persist_findings

    return persist_findings(app, job.job.payload["workspace"])


def compact_telemetry(app: Workgraph, job: JobContext) -> dict[str, Any]:
    from workgraph.jobs.retention import compact

    return compact(app, dry_run=bool(job.job.payload.get("dry_run")))


def noop(app: Workgraph, job: JobContext) -> dict[str, Any]:
    """Diagnostic job used by ``workgraph job enqueue noop`` to verify workers are alive."""
    import time

    seconds = float(job.job.payload.get("sleep", 0))
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        job.check_cancelled()
        time.sleep(min(0.1, seconds))
    if job.job.payload.get("fail"):
        raise RuntimeError(str(job.job.payload["fail"]))
    return {"ok": True, "attempt": job.job.attempt}


HANDLERS: dict[str, Handler] = {
    "git_sync": git_sync,
    "tracker_sync": tracker_sync,
    "analyze_commits": analyze_commits,
    "compute_change_surface": compute_change_surface,
    "classify_task": classify_task,
    "embed_task": embed_task,
    "evaluate_classifier": evaluate_classifier,
    "agent_run": agent_run,
    "verify": verify,
    "stack_integrate": stack_integrate,
    "stack_verify": stack_verify,
    "decompose_task": decompose_task,
    "automation_action": automation_action,
    "generate_report": generate_report,
    "failure_analysis": failure_analysis,
    "compact_telemetry": compact_telemetry,
    "noop": noop,
}
