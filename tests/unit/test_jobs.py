from __future__ import annotations

import threading
from datetime import timedelta

import pytest

from workgraph.core.clock import utcnow
from workgraph.core.errors import ConflictError
from workgraph.jobs.queue import JobQueue, backoff_delay
from workgraph.jobs.worker import Worker


def test_enqueue_idempotency_and_claim(app):
    queue = app.queue
    a = queue.enqueue("noop", {"x": 1}, idempotency_key="k1")
    b = queue.enqueue("noop", {"x": 2}, idempotency_key="k1")
    assert a == b
    job = queue.claim("w1")
    assert job is not None and job.id == a and job.attempt == 1
    assert queue.claim("w2") is None
    queue.complete(a, "w1", {"done": True})
    assert queue.get(a).status == "succeeded"
    with pytest.raises(ConflictError):
        queue.complete(a, "w1")


def test_retry_backoff_and_dead_letter(app):
    queue = app.queue
    job_id = queue.enqueue("noop", max_attempts=2, backoff_base_seconds=10)
    now = utcnow()
    job = queue.claim("w", now=now)
    assert job is not None
    assert queue.fail(job_id, "w", "boom") == "queued"
    row = queue.get(job_id)
    assert row.run_at > now + timedelta(seconds=9)
    assert queue.claim("w", now=now) is None  # backoff not elapsed
    job = queue.claim("w", now=now + timedelta(seconds=30))
    assert job is not None and job.attempt == 2
    assert queue.fail(job_id, "w", "boom again") == "dead"
    assert queue.get(job_id).status == "dead"
    queue.retry_dead(job_id)
    assert queue.get(job_id).status == "queued"
    assert backoff_delay(2, 1) == 2 and backoff_delay(2, 4) == 16


def test_non_retryable_failure_goes_dead_immediately(app):
    job_id = app.queue.enqueue("noop", max_attempts=5)
    app.queue.claim("w")
    assert app.queue.fail(job_id, "w", "bad input", retryable=False) == "dead"


def test_lease_expiry_recovery(app):
    queue = app.queue
    job_id = queue.enqueue("noop", max_attempts=3)
    now = utcnow()
    assert queue.claim("crashed-worker", lease_seconds=5, now=now) is not None
    assert queue.recover_expired(now + timedelta(seconds=2)) == []
    assert queue.recover_expired(now + timedelta(seconds=10)) == [job_id]
    row = queue.get(job_id)
    assert row.status == "queued" and row.error_kind == "lease_expired"
    with pytest.raises(ConflictError):
        queue.heartbeat(job_id, "crashed-worker")


def test_resource_limits(app):
    config = app.config
    config.jobs.gpu_concurrency = 1
    app.config_store.save_global(config)
    queue = app.queue
    first = queue.enqueue("noop", resource_keys=["gpu"])
    second = queue.enqueue("noop", resource_keys=["gpu"])
    other = queue.enqueue("noop", resource_keys=["repo:abc"])
    claimed = queue.claim("w")
    assert claimed is not None and claimed.id == first
    nxt = queue.claim("w")
    assert nxt is not None and nxt.id == other  # gpu slot busy; independent job proceeds
    assert queue.claim("w") is None
    queue.complete(first, "w")
    again = queue.claim("w")
    assert again is not None and again.id == second


def test_cancellation(app):
    queue = app.queue
    queued = queue.enqueue("noop")
    assert queue.cancel(queued) == "cancelled"
    running = queue.enqueue("noop")
    queue.claim("w")
    assert queue.cancel(running) == "running"
    assert queue.heartbeat(running, "w") is True


def test_concurrent_claims_never_duplicate(app):
    for i in range(40):
        app.queue.enqueue("noop", {"i": i})
    claimed: list[str] = []
    lock = threading.Lock()

    def worker(name: str) -> None:
        q = JobQueue(app.db)
        while True:
            job = q.claim(name)
            if job is None:
                return
            with lock:
                claimed.append(job.id)
            q.complete(job.id, name)

    threads = [threading.Thread(target=worker, args=(f"w{i}",)) for i in range(6)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert len(claimed) == 40 and len(set(claimed)) == 40


def test_worker_runs_handlers_retries_and_dead_letters(app):
    ok = app.queue.enqueue("noop", {"sleep": 0})
    bad = app.queue.enqueue("noop", {"fail": "always"}, max_attempts=2, backoff_base_seconds=0)
    unknown = app.queue.enqueue("does_not_exist")
    worker = Worker(app, concurrency=2, poll_interval=0.05, maintenance_interval=0.2)
    worker.run_until_idle(timeout=60)
    assert app.queue.get(ok).status == "succeeded"
    assert app.queue.get(bad).status == "dead"
    assert len(app.queue.runs(bad)) == 2
    assert app.queue.get(unknown).status == "dead"
    workers = app.queue.workers()
    assert workers[0].status == "stopped"


def test_worker_cancels_running_job(app):
    job_id = app.queue.enqueue("noop", {"sleep": 30})
    worker = Worker(app, concurrency=1, poll_interval=0.05)
    worker.lease_seconds = 3
    thread = threading.Thread(target=worker.run, kwargs={"max_idle_seconds": 15, "grace_seconds": 10})
    thread.start()
    import time

    for _ in range(100):
        if app.queue.get(job_id).status == "running":
            break
        time.sleep(0.05)
    app.queue.cancel(job_id)
    for _ in range(200):
        if app.queue.get(job_id).status == "cancelled":
            break
        time.sleep(0.05)
    worker.stop()
    thread.join(timeout=20)
    assert app.queue.get(job_id).status == "cancelled"
