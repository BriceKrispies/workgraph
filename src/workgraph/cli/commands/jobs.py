"""worker and job commands."""

from __future__ import annotations

import json
from typing import Any

import typer

from workgraph.cli.common import STATE, console, emit, handle_errors, kv, short, table, when

worker_app = typer.Typer(help="Run and inspect background workers.", no_args_is_help=True)
job_app = typer.Typer(help="Inspect and control the durable job queue.", no_args_is_help=True)


@worker_app.command("run")
@handle_errors
def worker_run(
    concurrency: int | None = typer.Option(None, help="Parallel jobs (default from config)"),
    kind: list[str] = typer.Option(None, "--kind", help="Only run these job kinds (repeatable)"),
    once: bool = typer.Option(False, help="Process runnable jobs and exit"),
    max_idle: float | None = typer.Option(None, help="Exit after this many idle seconds"),
) -> None:
    """Run a worker until interrupted (Ctrl+C stops gracefully)."""
    from workgraph.core.logging import configure_logging
    from workgraph.jobs.worker import Worker

    app = STATE.app()
    configure_logging("INFO", app.paths.log_dir)
    worker = Worker(app, concurrency=concurrency, kinds=kind or None)
    worker.install_signal_handlers()
    console.print(f"worker {worker.worker_id} started (concurrency {worker.concurrency})")
    worker.run(once=once, max_idle_seconds=max_idle)
    emit({"worker": worker.worker_id, "completed": worker.completed, "failed": worker.failed},
         lambda r: console.print(f"worker stopped: {r['completed']} completed, {r['failed']} failed"))


@worker_app.command("list")
@handle_errors
def worker_list() -> None:
    """Show workers and their heartbeats."""
    workers = STATE.app().queue.workers()
    rows = [{"id": w.id, "status": w.status, "pid": w.pid, "concurrency": w.concurrency, "current": w.current_jobs,
             "completed": w.jobs_completed, "failed": w.jobs_failed, "heartbeat_at": w.heartbeat_at} for w in workers]
    emit(rows, lambda rs: table([(r["id"], r["status"], r["pid"], r["concurrency"], len(r["current"]), r["completed"],
                                  r["failed"], when(r["heartbeat_at"])) for r in rs],
                                ["worker", "status", "pid", "conc", "running", "done", "failed", "heartbeat"]))


@worker_app.command("recover")
@handle_errors
def worker_recover() -> None:
    """Recover expired job leases, lost workers and orphaned worktrees now."""
    from workgraph.core.context import RequestContext

    app = STATE.app()
    result = {"recovered_jobs": app.queue.recover_expired(), "lost_workers": app.queue.mark_lost_workers(),
              "worktrees": app.worktrees.recover(RequestContext.system("cli"))}
    emit(result, lambda r: kv(r.items(), title="recovery"))


def _job_row(j: Any) -> tuple[Any, ...]:
    return (short(j.id), j.kind, j.status, f"{j.attempts}/{j.max_attempts}", when(j.run_at),
            (j.last_error or "")[:60])


@job_app.command("list")
@handle_errors
def job_list(status: list[str] = typer.Option(None, "--status", "-s"), kind: str | None = typer.Option(None),
             limit: int = typer.Option(50), offset: int = typer.Option(0)) -> None:
    """List jobs (newest first)."""
    jobs, total = STATE.app().queue.list(statuses=status or None, kind=kind, limit=limit, offset=offset)
    data = {"total": total, "items": [{c: getattr(j, c) for c in ("id", "kind", "status", "attempts", "max_attempts",
                                                                   "run_at", "last_error", "error_kind", "task_id",
                                                                   "attempt_id", "created_at")} for j in jobs]}
    emit(data, lambda _: table([_job_row(j) for j in jobs], ["job", "kind", "status", "attempts", "run at", "error"]))


@job_app.command("show")
@handle_errors
def job_show(job_id: str) -> None:
    """Show a job, its payload, result and run history."""
    app = STATE.app()
    job = app.queue.get(job_id)
    runs = app.queue.runs(job_id)
    data = {**{c: getattr(job, c) for c in ("id", "kind", "status", "payload", "result", "attempts", "max_attempts",
                                            "run_at", "lease_owner", "lease_expires_at", "last_error", "error_kind",
                                            "resource_keys", "cancel_requested", "created_at", "finished_at")},
            "runs": [{"attempt": r.attempt_number, "worker": r.worker_id, "status": r.status, "error": r.error,
                      "started_at": r.started_at, "finished_at": r.finished_at} for r in runs]}
    emit(data, lambda d: (kv([(k, v) for k, v in d.items() if k != "runs"], title=job.kind),
                          table([(r["attempt"], r["worker"], r["status"], (r["error"] or "")[:80]) for r in d["runs"]],
                                ["#", "worker", "status", "error"])))


@job_app.command("stats")
@handle_errors
def job_stats() -> None:
    """Counts by status."""
    stats = STATE.app().queue.stats()
    emit(stats, lambda s: kv(s.items(), title="jobs"))


@job_app.command("cancel")
@handle_errors
def job_cancel(job_id: str) -> None:
    """Cancel a queued job, or request cancellation of a running one."""
    status = STATE.app().queue.cancel(job_id)
    emit({"job_id": job_id, "status": status}, lambda r: console.print(f"{job_id}: {status}"))


@job_app.command("retry")
@handle_errors
def job_retry(job_id: str) -> None:
    """Re-queue a dead-lettered or cancelled job."""
    STATE.app().queue.retry_dead(job_id)
    emit({"job_id": job_id, "status": "queued"}, lambda r: console.print(f"{job_id} re-queued"))


@job_app.command("enqueue")
@handle_errors
def job_enqueue(kind: str, payload: str = typer.Option("{}", help="JSON payload"),
                key: str | None = typer.Option(None, help="Idempotency key")) -> None:
    """Enqueue a job (e.g. `noop` to check that workers are alive, `git_sync`, `compact_telemetry`)."""
    from workgraph.jobs.handlers import HANDLERS

    if kind not in HANDLERS:
        from workgraph.cli.common import fail

        fail(f"unknown job kind {kind!r}; known: {', '.join(sorted(HANDLERS))}", 2)
    job_id = STATE.app().queue.enqueue(kind, json.loads(payload), idempotency_key=key, created_by="human:cli")
    emit({"job_id": job_id}, lambda r: console.print(f"enqueued {job_id}"))


@job_app.command("compact")
@handle_errors
def job_compact(dry_run: bool = typer.Option(True, "--dry-run/--apply")) -> None:
    """Apply telemetry retention now (hook payloads and run logs past the retention window)."""
    from workgraph.jobs.retention import compact

    emit(compact(STATE.app(), dry_run=dry_run), lambda r: kv(r.items(), title="retention"))
