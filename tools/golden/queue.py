"""Golden vectors GV-20 (``queue``) for the job queue.

Drives ``workgraph.jobs.queue`` and ``workgraph.jobs.limits`` through a temporary SQLite
database with a fixed clock, and writes the observed behaviour to ``golden/queue/*.json``.

Determinism:

* ``workgraph.core.clock.utcnow`` and ``new_id`` are replaced before any ``workgraph``
  module is imported, so every ORM default and every queue timestamp comes from the fixed
  clock and a counter.
* Job ids never reach the output: each job is written under the label given at enqueue.
* Timestamps are written as seconds after the fixed epoch.

Run with ``uv run python tools/golden/queue.py``; the output bytes do not depend on the
machine, the time of day or the temporary directory.
"""

from __future__ import annotations

import json
import shutil
import tempfile
from datetime import UTC, datetime, timedelta
from pathlib import Path
from types import SimpleNamespace
from typing import Any

EPOCH = datetime(2026, 1, 1, 0, 0, 0, tzinfo=UTC)
ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "queue"


class FixedClock:
    """Stands in for ``workgraph.core.clock``: one settable instant and a counter for ids."""

    def __init__(self) -> None:
        self.now = EPOCH
        self.seq = 0

    def utcnow(self) -> datetime:
        return self.now

    def new_id(self) -> str:
        self.seq += 1
        return f"00000000-0000-4000-8000-{self.seq:012d}"

    def at(self, seconds: float) -> datetime:
        self.now = EPOCH + timedelta(seconds=seconds)
        return self.now


CLOCK = FixedClock()
LABELS: dict[str, str] = {}


def _off(value: datetime | None) -> float | None:
    if value is None:
        return None
    if value.tzinfo is None:
        value = value.replace(tzinfo=UTC)
    return (value - EPOCH).total_seconds()


def _label(job_id: str | None) -> str | None:
    if job_id is None:
        return None
    return LABELS.get(job_id, "unlabelled")


def _error(exc: BaseException) -> dict[str, str]:
    return {"error": type(exc).__name__}


class Harness:
    """One fresh temporary database per case group, wired with the real limit resolver."""

    def __init__(self, cfg: Any) -> None:
        from workgraph.jobs.limits import resource_limit_resolver
        from workgraph.jobs.queue import JobQueue
        from workgraph.persistence.db import Database
        from workgraph.persistence.migrate import upgrade

        self.tmp = Path(tempfile.mkdtemp(prefix="wg-gv20-"))
        self.db = Database(self.tmp / "workgraph.db")
        upgrade(self.db)
        app = SimpleNamespace(config=cfg)
        self.queue = JobQueue(self.db, limit_for=resource_limit_resolver(app))

    def close(self) -> None:
        self.db.dispose()
        shutil.rmtree(self.tmp, ignore_errors=True)

    def enqueue(self, label: str, kind: str = "noop", **kwargs: Any) -> str:
        job_id = self.queue.enqueue(kind, kwargs.pop("payload", None), **kwargs)
        LABELS[job_id] = label
        return job_id

    def claim(self, worker: str, now: datetime, **kwargs: Any) -> dict[str, Any]:
        job = self.queue.claim(worker, now=now, **kwargs)
        if job is None:
            return {"op": "claim", "worker": worker, "at": _off(now), "got": None}
        return {
            "op": "claim",
            "worker": worker,
            "at": _off(now),
            "got": _label(job.id),
            "attempt": job.attempt,
            "lease_expires_at": _off(job.lease_expires_at),
        }

    def row(self, job_id: str) -> dict[str, Any]:
        job = self.queue.get(job_id)
        return {
            "label": _label(job_id),
            "status": str(job.status),
            "attempts": job.attempts,
            "max_attempts": job.max_attempts,
            "run_at": _off(job.run_at),
            "finished_at": _off(job.finished_at),
            "error_kind": job.error_kind,
            "last_error": job.last_error,
            "cancel_requested": job.cancel_requested,
            "resource_keys": list(job.resource_keys),
        }

    def runs(self, job_id: str) -> list[dict[str, Any]]:
        return [
            {
                "attempt_number": run.attempt_number,
                "worker": run.worker_id,
                "status": str(run.status),
                "error": run.error,
                "finished_at": _off(run.finished_at),
            }
            for run in self.queue.runs(job_id)
        ]


def _config() -> SimpleNamespace:
    """Limits as the configuration defaults the golden cases rely on, read by the real resolver."""
    return SimpleNamespace(
        jobs=SimpleNamespace(
            gpu_concurrency=1,
            per_agent_concurrency=1,
            per_repository_concurrency=1,
            verify_concurrency=2,
        ),
        execution=SimpleNamespace(max_concurrent_runs=2),
    )


def _attempt(fn: Any) -> dict[str, Any]:
    try:
        return {"ok": fn()}
    except Exception as exc:
        return _error(exc)


# ---------------------------------------------------------------- case groups


def claim_order_cases() -> list[dict[str, Any]]:
    from workgraph.jobs import limits

    h = Harness(_config())
    try:
        steps: list[dict[str, Any]] = []
        CLOCK.at(0)
        h.enqueue("gpu-1", resource_keys=["gpu"])
        CLOCK.at(1)
        h.enqueue("gpu-2", resource_keys=["gpu"])
        CLOCK.at(2)
        h.enqueue("agent-alice-1", resource_keys=[limits.agent_key("alice")])
        CLOCK.at(3)
        h.enqueue("repo-r1-1", priority=50, resource_keys=[limits.repo_key("r1")])
        CLOCK.at(4)
        h.enqueue("verify-r1-1", resource_keys=[limits.verify_key("r1"), limits.repo_key("r1")])
        CLOCK.at(5)
        h.enqueue("tracker-c1-1", resource_keys=[limits.tracker_key("c1")])
        CLOCK.at(6)
        h.enqueue("stack-s1-r1-1", resource_keys=[limits.stack_key("s1", "r1")])
        CLOCK.at(7)
        h.enqueue("unlimited-1", priority=200)
        CLOCK.at(8)
        h.enqueue("verify-r2-1", resource_keys=[limits.verify_key("r2")])
        CLOCK.at(9)
        h.enqueue("verify-r2-2", resource_keys=[limits.verify_key("r2")])
        CLOCK.at(10)
        h.enqueue("verify-r2-3", resource_keys=[limits.verify_key("r2")])
        CLOCK.at(11)
        h.enqueue("gpu-3", resource_keys=["gpu", "run"])
        CLOCK.at(12)
        h.enqueue("future", priority=1, run_at=EPOCH + timedelta(seconds=600))

        now = CLOCK.at(20)
        # Drain every runnable job with nothing completed: the limits decide what is taken.
        for _ in range(12):
            step = h.claim("w1", now)
            steps.append(step)
            if step["got"] is None:
                break
        steps.append({"op": "claim_peek", "got": _label(h.queue.claim_peek())})

        # Completing the GPU holder frees the gpu slot for the next gpu job.
        first_gpu = next(j for j in list(_all_ids(h)) if LABELS[j] == "gpu-1")
        steps.append(
            {
                "op": "complete",
                "label": "gpu-1",
                "result": _attempt(lambda: h.queue.complete(first_gpu, "w1")),
            }
        )
        steps.append(h.claim("w1", now))

        # Completing the repository job frees repo:r1, which unblocks the verify job.
        repo_job = next(j for j in list(_all_ids(h)) if LABELS[j] == "repo-r1-1")
        steps.append(
            {
                "op": "complete",
                "label": "repo-r1-1",
                "result": _attempt(lambda: h.queue.complete(repo_job, "w1")),
            }
        )
        steps.append(h.claim("w1", now))

        # Jobs not yet eligible wait for their run_at; the kind filter applies first.
        later = CLOCK.at(600)
        h.enqueue("report-1", kind="report")
        steps.append(h.claim("w2", later, kinds=["noop"]))
        steps.append(h.claim("w2", later, kinds=["report"]))
        steps.append(h.claim("w2", later))
        steps.append(h.claim("w2", later))
    finally:
        h.close()
    return [{"name": "limits_and_order", "steps": steps}]


def _all_ids(h: Harness) -> list[str]:
    jobs, _total = h.queue.list(limit=200)
    return [job.id for job in jobs]


def backoff_cases() -> list[dict[str, Any]]:
    from workgraph.jobs.queue import backoff_delay

    table = []
    for base in (0.0, 0.5, 2.0, 10.0):
        for attempt in (0, 1, 2, 3, 4, 8, 12, 13):
            table.append({"base": base, "attempt": attempt, "delay": backoff_delay(base, attempt)})
    capped = [
        {"base": base, "attempt": attempt, "cap": 30.0, "delay": backoff_delay(base, attempt, 30.0)}
        for base in (2.0, 10.0)
        for attempt in (1, 2, 3, 4, 5)
    ]

    h = Harness(_config())
    try:
        steps: list[dict[str, Any]] = []
        CLOCK.at(0)
        job_id = h.enqueue("retry-job", max_attempts=4, backoff_base_seconds=2.0)
        for _ in range(4):
            now = CLOCK.now
            steps.append(h.claim("w", now))
            status = h.queue.fail(job_id, "w", f"boom at {_off(now)}")
            row = h.queue.get(job_id)
            run_at = row.run_at
            steps.append({"op": "fail", "status": str(status), "row": h.row(job_id)})
            if str(status) != "queued":
                break
            # Just before run_at nothing is claimable; at run_at it is.
            steps.append(h.claim("w", run_at - timedelta(seconds=1)))
            CLOCK.now = run_at
        steps.append({"op": "runs", "runs": h.runs(job_id)})

        # Zero base: retried job is immediately claimable.
        CLOCK.at(100)
        zero = h.enqueue("zero-base", max_attempts=3, backoff_base_seconds=0.0)
        h.queue.claim("w", now=CLOCK.now)
        steps.append({"op": "fail", "status": str(h.queue.fail(zero, "w", "x")), "row": h.row(zero)})
        steps.append(h.claim("w", CLOCK.now))
    finally:
        h.close()
    return [
        {"name": "backoff_delay_table", "vectors": table},
        {"name": "backoff_delay_capped", "vectors": capped},
        {"name": "retry_schedule", "steps": steps},
    ]


def lease_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []

    # Expiry boundary: a lease is recovered only strictly after lease_expires_at.
    h = Harness(_config())
    try:
        steps: list[dict[str, Any]] = []
        CLOCK.at(0)
        job_id = h.enqueue("lease-job", max_attempts=3)
        steps.append(h.claim("crashed", CLOCK.now, lease_seconds=5))
        steps.append(
            {"op": "recover", "at": 5, "ids": [_label(j) for j in h.queue.recover_expired(CLOCK.at(5))]}
        )
        recovered = h.queue.recover_expired(CLOCK.at(6))
        steps.append({"op": "recover", "at": 6, "ids": [_label(j) for j in recovered]})
        steps.append({"op": "row", "row": h.row(job_id)})
        steps.append(
            {
                "op": "heartbeat",
                "worker": "crashed",
                "result": _attempt(lambda: h.queue.heartbeat(job_id, "crashed")),
            }
        )
        steps.append(h.claim("w2", CLOCK.now))
        steps.append({"op": "runs", "runs": h.runs(job_id)})
        cases.append({"name": "expiry_boundary_and_redelivery", "steps": steps})
    finally:
        h.close()

    # Heartbeats extend the lease and report cancellation.
    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        job_id = h.enqueue("heartbeat-job", max_attempts=3)
        steps.append(h.claim("w1", CLOCK.now, lease_seconds=5))
        CLOCK.at(4)
        steps.append(
            {"op": "heartbeat", "at": 4, "result": _attempt(lambda: h.queue.heartbeat(job_id, "w1", 10))}
        )
        steps.append({"op": "row", "row": h.row(job_id)})
        steps.append(
            {"op": "recover", "at": 13, "ids": [_label(j) for j in h.queue.recover_expired(CLOCK.at(13))]}
        )
        steps.append(
            {"op": "recover", "at": 15, "ids": [_label(j) for j in h.queue.recover_expired(CLOCK.at(15))]}
        )
        steps.append({"op": "row", "row": h.row(job_id)})
        cases.append({"name": "heartbeat_extends_lease", "steps": steps})
    finally:
        h.close()

    # Lease expiry on the last attempt goes to the dead letter.
    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        job_id = h.enqueue("last-attempt", max_attempts=1)
        steps.append(h.claim("w1", CLOCK.now, lease_seconds=5))
        CLOCK.at(10)
        steps.append(
            {"op": "recover", "at": 10, "ids": [_label(j) for j in h.queue.recover_expired(CLOCK.now)]}
        )
        steps.append({"op": "row", "row": h.row(job_id)})
        steps.append({"op": "runs", "runs": h.runs(job_id)})
        cases.append({"name": "expiry_on_last_attempt_is_dead", "steps": steps})
    finally:
        h.close()

    # Only the lease owner may heartbeat or complete; unknown ids fail too.
    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        job_id = h.enqueue("owned-job")
        steps.append(h.claim("w1", CLOCK.now))
        steps.append(
            {"op": "heartbeat", "worker": "w2", "result": _attempt(lambda: h.queue.heartbeat(job_id, "w2"))}
        )
        steps.append(
            {"op": "complete", "worker": "w2", "result": _attempt(lambda: h.queue.complete(job_id, "w2"))}
        )
        unknown = "00000000-0000-4000-8000-999999999999"
        steps.append(
            {
                "op": "heartbeat",
                "worker": "w1",
                "job": "unknown",
                "result": _attempt(lambda: h.queue.heartbeat(unknown, "w1")),
            }
        )
        steps.append({"op": "get", "job": "unknown", "result": _attempt(lambda: h.queue.get(unknown).status)})
        steps.append({"op": "row", "row": h.row(job_id)})
        cases.append({"name": "ownership_checks", "steps": steps})
    finally:
        h.close()
    return cases


def dead_letter_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    h = Harness(_config())
    try:
        steps: list[dict[str, Any]] = []
        CLOCK.at(0)
        job_id = h.enqueue("two-tries", max_attempts=2, backoff_base_seconds=10.0)
        steps.append(h.claim("w", CLOCK.now))
        steps.append(
            {"op": "fail", "status": str(h.queue.fail(job_id, "w", "first failure")), "row": h.row(job_id)}
        )
        CLOCK.at(10)
        steps.append(h.claim("w", CLOCK.now))
        steps.append(
            {"op": "fail", "status": str(h.queue.fail(job_id, "w", "second failure")), "row": h.row(job_id)}
        )
        steps.append({"op": "runs", "runs": h.runs(job_id)})
        CLOCK.at(20)
        steps.append({"op": "retry_dead", "result": _attempt(lambda: h.queue.retry_dead(job_id))})
        steps.append({"op": "row", "row": h.row(job_id)})
        steps.append(
            {"op": "retry_dead", "note": "queued job", "result": _attempt(lambda: h.queue.retry_dead(job_id))}
        )
        steps.append(h.claim("w", CLOCK.now))
        steps.append({"op": "stats", "stats": h.queue.stats()})
        cases.append({"name": "retry_exhaustion_then_manual_retry", "steps": steps})
    finally:
        h.close()

    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        job_id = h.enqueue("non-retryable", max_attempts=5)
        steps.append(h.claim("w", CLOCK.now))
        steps.append(
            {
                "op": "fail",
                "status": str(
                    h.queue.fail(job_id, "w", "bad input", retryable=False, error_kind="validation")
                ),
                "row": h.row(job_id),
            }
        )
        steps.append(h.claim("w", CLOCK.now))
        cases.append({"name": "non_retryable_goes_dead", "steps": steps})
    finally:
        h.close()

    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        job_id = h.enqueue("long-error", max_attempts=5)
        steps.append(h.claim("w", CLOCK.now))
        steps.append({"op": "fail", "status": str(h.queue.fail(job_id, "w", "x" * 25_000))})
        row = h.queue.get(job_id)
        steps.append(
            {
                "op": "lengths",
                "last_error": len(row.last_error or ""),
                "run_error": len(h.runs(job_id)[0]["error"] or ""),
            }
        )
        cases.append({"name": "error_text_truncation", "steps": steps})
    finally:
        h.close()

    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        job_id = h.enqueue("cancel-during-run", max_attempts=5)
        steps.append(h.claim("w", CLOCK.now))
        steps.append({"op": "cancel", "status": str(h.queue.cancel(job_id))})
        steps.append({"op": "fail", "status": str(h.queue.fail(job_id, "w", "boom")), "row": h.row(job_id)})
        steps.append({"op": "runs", "runs": h.runs(job_id)})
        steps.append(
            {
                "op": "retry_dead",
                "note": "cancelled job",
                "result": _attempt(lambda: h.queue.retry_dead(job_id)),
            }
        )
        steps.append({"op": "row", "row": h.row(job_id)})
        cases.append({"name": "cancel_requested_failure_is_cancelled", "steps": steps})
    finally:
        h.close()
    return cases


def idempotency_cases() -> list[dict[str, Any]]:
    h = Harness(_config())
    try:
        steps: list[dict[str, Any]] = []
        CLOCK.at(0)
        first = h.enqueue("first", payload={"x": 1}, idempotency_key="k1")
        again = h.queue.enqueue("noop", {"x": 2}, idempotency_key="k1")
        steps.append(
            {
                "op": "enqueue",
                "key": "k1",
                "same_job": again == first,
                "label": _label(again),
                "payload": h.queue.get(first).payload,
            }
        )
        steps.append(h.claim("w", CLOCK.now))
        h.queue.complete(first, "w", {"done": True})
        after = h.queue.enqueue("noop", {"x": 3}, idempotency_key="k1")
        steps.append(
            {
                "op": "enqueue_after_success",
                "key": "k1",
                "same_job": after == first,
                "status": str(h.queue.get(first).status),
            }
        )
        jobs, total = h.queue.list(limit=50)
        steps.append({"op": "list", "total": total, "labels": sorted(_label(j.id) for j in jobs)})

        keep = h.enqueue("keep-resources", idempotency_key="k2", resource_keys=["gpu"])
        dup = h.queue.enqueue("noop", idempotency_key="k2", resource_keys=[])
        steps.append(
            {
                "op": "enqueue",
                "key": "k2",
                "same_job": dup == keep,
                "resource_keys": h.row(keep)["resource_keys"],
            }
        )

        kind_keep = h.enqueue("report-kind", kind="report", idempotency_key="k3")
        kind_dup = h.queue.enqueue("noop", idempotency_key="k3")
        steps.append(
            {
                "op": "enqueue",
                "key": "k3",
                "same_job": kind_dup == kind_keep,
                "kind": h.queue.get(kind_keep).kind,
            }
        )

        cancelled = h.enqueue("cancel-me", idempotency_key="k4")
        h.queue.cancel(cancelled)
        dup_cancelled = h.queue.enqueue("noop", idempotency_key="k4")
        steps.append(
            {
                "op": "enqueue_after_cancel",
                "key": "k4",
                "same_job": dup_cancelled == cancelled,
                "status": str(h.queue.get(cancelled).status),
            }
        )

        anon_a = h.enqueue("anon-a")
        anon_b = h.enqueue("anon-b")
        steps.append(
            {
                "op": "enqueue_without_key",
                "distinct": anon_a != anon_b,
                "labels": [_label(anon_a), _label(anon_b)],
            }
        )
    finally:
        h.close()
    return [{"name": "idempotency_keys", "steps": steps}]


def cancellation_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    h = Harness(_config())
    try:
        steps: list[dict[str, Any]] = []
        CLOCK.at(0)
        queued = h.enqueue("queued-cancel")
        steps.append({"op": "cancel", "status": str(h.queue.cancel(queued)), "row": h.row(queued)})
        steps.append(h.claim("w", CLOCK.now))
        steps.append({"op": "claim_peek", "got": _label(h.queue.claim_peek())})
        cases.append({"name": "queued_cancel_is_never_claimed", "steps": steps})
    finally:
        h.close()

    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        running = h.enqueue("running-complete")
        steps.append(h.claim("w1", CLOCK.now))
        steps.append({"op": "cancel", "status": str(h.queue.cancel(running))})
        steps.append({"op": "heartbeat", "result": _attempt(lambda: h.queue.heartbeat(running, "w1"))})
        steps.append(
            {"op": "complete", "result": _attempt(lambda: h.queue.complete(running, "w1", {"ok": True}))}
        )
        steps.append({"op": "row", "row": h.row(running)})
        steps.append({"op": "runs", "runs": h.runs(running)})
        cases.append({"name": "running_cancel_is_advisory_until_worker_reports", "steps": steps})
    finally:
        h.close()

    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        operator = h.enqueue("operator-stop")
        steps.append(h.claim("w1", CLOCK.now))
        steps.append(
            {"op": "mark_cancelled", "wrong_worker": _attempt(lambda: h.queue.mark_cancelled(operator, "w2"))}
        )
        steps.append(
            {
                "op": "mark_cancelled",
                "result": _attempt(lambda: h.queue.mark_cancelled(operator, "w1", "stopped by operator")),
            }
        )
        steps.append({"op": "row", "row": h.row(operator)})
        steps.append({"op": "runs", "runs": h.runs(operator)})
        cases.append({"name": "mark_cancelled_records_reason", "steps": steps})
    finally:
        h.close()

    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        expired = h.enqueue("cancel-then-expire", max_attempts=5)
        steps.append(h.claim("w1", CLOCK.now, lease_seconds=5))
        steps.append({"op": "cancel", "status": str(h.queue.cancel(expired))})
        CLOCK.at(10)
        steps.append({"op": "recover", "ids": [_label(j) for j in h.queue.recover_expired(CLOCK.now)]})
        steps.append({"op": "row", "row": h.row(expired)})
        cases.append({"name": "cancel_requested_expiry_is_cancelled", "steps": steps})
    finally:
        h.close()

    h = Harness(_config())
    try:
        steps = []
        CLOCK.at(0)
        done = h.enqueue("already-done")
        steps.append(h.claim("w", CLOCK.now))
        h.queue.complete(done, "w")
        steps.append({"op": "cancel_succeeded", "status": str(h.queue.cancel(done))})
        steps.append(
            {
                "op": "cancel_unknown",
                "result": _attempt(lambda: h.queue.cancel("00000000-0000-4000-8000-999999999999")),
            }
        )
        steps.append({"op": "row", "row": h.row(done)})
        cases.append({"name": "cancel_terminal_and_unknown", "steps": steps})
    finally:
        h.close()
    return cases


GROUPS = {
    "claim_order": claim_order_cases,
    "backoff": backoff_cases,
    "leases": lease_cases,
    "dead_letter": dead_letter_cases,
    "idempotency": idempotency_cases,
    "cancellation": cancellation_cases,
}


def _install_fixed_clock() -> None:
    """Replace the clock before any workgraph module binds ``utcnow`` or ``new_id``."""
    import workgraph.core.clock as clock_module

    clock_module.utcnow = CLOCK.utcnow
    clock_module.new_id = CLOCK.new_id


def main() -> None:
    _install_fixed_clock()
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for name, build in GROUPS.items():
        LABELS.clear()
        CLOCK.now = EPOCH
        CLOCK.seq = 0
        document = {"group": name, "cases": build()}
        text = json.dumps(document, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
        (OUT_DIR / f"{name}.json").write_bytes(text.encode("utf-8"))


if __name__ == "__main__":
    main()
