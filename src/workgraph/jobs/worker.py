"""Background worker: claims jobs from the durable queue and runs their handlers.

* Each running job has a heartbeat thread that extends its lease and relays
  cancellation requests; a crashed worker simply stops heartbeating and its jobs
  are recovered by another worker once the lease expires.
* One worker at a time performs periodic maintenance (lease recovery, automation
  event dispatch, scheduled rules, the agent scheduler, hook-spool replay). It is
  elected through an expiring ``resource_leases`` row, so several workers may run.
* Shutdown is graceful: no new jobs are claimed and running jobs get a grace period.
"""

from __future__ import annotations

import os
import platform
import signal
import threading
import time
import uuid
from collections.abc import Callable
from concurrent.futures import Future, ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from workgraph.core.errors import (
    ConflictError,
    NotFoundError,
    PermissionDenied,
    ProviderUnavailableError,
    ValidationFailed,
    WorkgraphError,
)
from workgraph.core.logging import get_logger
from workgraph.jobs.queue import ClaimedJob

if TYPE_CHECKING:
    from workgraph.app import Workgraph

log = get_logger("worker")


class JobCancelled(Exception):
    """Raised by handlers that observe a cancellation request."""


@dataclass
class JobContext:
    job: ClaimedJob
    worker_id: str
    _cancel: threading.Event = field(default_factory=threading.Event)

    def cancel_requested(self) -> bool:
        return self._cancel.is_set()

    def check_cancelled(self) -> None:
        if self._cancel.is_set():
            raise JobCancelled(f"job {self.job.id} cancelled")


Handler = Callable[["Workgraph", JobContext], dict[str, Any] | None]

NON_RETRYABLE = (ValidationFailed, NotFoundError, PermissionDenied)


class Worker:
    def __init__(
        self,
        app: Workgraph,
        *,
        concurrency: int | None = None,
        kinds: list[str] | None = None,
        poll_interval: float = 1.0,
        maintenance_interval: float = 5.0,
        worker_id: str | None = None,
        handlers: dict[str, Handler] | None = None,
    ) -> None:
        from workgraph.jobs.handlers import HANDLERS

        self.app = app
        config = app.config.jobs
        self.concurrency = concurrency or config.worker_concurrency
        self.lease_seconds = config.lease_seconds
        self.kinds = kinds
        self.poll_interval = poll_interval
        self.maintenance_interval = maintenance_interval
        self.worker_id = worker_id or f"{platform.node()}-{os.getpid()}-{uuid.uuid4().hex[:6]}"
        self.handlers = handlers or HANDLERS
        self._stop = threading.Event()
        self._executor = ThreadPoolExecutor(max_workers=self.concurrency, thread_name_prefix="wg-job")
        self._running: dict[str, Future[None]] = {}
        self._lock = threading.Lock()
        self.completed = 0
        self.failed = 0
        self._last_maintenance = 0.0
        self._last_heartbeat = 0.0

    # ------------------------------------------------------------------ lifecycle

    def install_signal_handlers(self) -> None:
        def stop(_signum: int, _frame: object) -> None:
            log.info("shutdown requested", extra={"worker": self.worker_id})
            self._stop.set()

        signal.signal(signal.SIGINT, stop)
        if hasattr(signal, "SIGTERM"):
            signal.signal(signal.SIGTERM, stop)

    def stop(self) -> None:
        self._stop.set()

    def run(
        self, *, once: bool = False, max_idle_seconds: float | None = None, grace_seconds: float = 30.0
    ) -> None:
        """Run until stopped. ``once`` drains currently runnable jobs and returns."""
        queue = self.app.queue
        queue.register_worker(
            self.worker_id, platform.node(), os.getpid(), self.concurrency, self.kinds or []
        )
        log.info("worker started", extra={"worker": self.worker_id, "concurrency": self.concurrency})
        self._startup_recovery()
        idle_since = time.monotonic()
        try:
            while not self._stop.is_set():
                self._maybe_maintenance()
                self._reap()
                claimed_any = False
                while len(self._running) < self.concurrency and not self._stop.is_set():
                    job = queue.claim(self.worker_id, kinds=self.kinds, lease_seconds=self.lease_seconds)
                    if job is None:
                        break
                    claimed_any = True
                    self._start(job)
                if claimed_any or self._running:
                    idle_since = time.monotonic()
                if once and not self._running and not claimed_any:
                    break
                if max_idle_seconds is not None and time.monotonic() - idle_since > max_idle_seconds:
                    break
                self._heartbeat_worker()
                self._stop.wait(self.poll_interval if not claimed_any else 0.05)
        finally:
            self._shutdown(grace_seconds)

    def run_until_idle(self, timeout: float = 120.0) -> None:
        """Process jobs until the queue has nothing runnable (used by tests and demos)."""
        deadline = time.monotonic() + timeout
        queue = self.app.queue
        queue.register_worker(
            self.worker_id, platform.node(), os.getpid(), self.concurrency, self.kinds or []
        )
        try:
            while time.monotonic() < deadline:
                self._maybe_maintenance(force=True)
                self._reap()
                started = False
                while len(self._running) < self.concurrency:
                    job = queue.claim(self.worker_id, kinds=self.kinds, lease_seconds=self.lease_seconds)
                    if job is None:
                        break
                    self._start(job)
                    started = True
                if not started and not self._running:
                    self._maybe_maintenance(force=True)
                    if queue.claim_peek(kinds=self.kinds) is None:
                        return
                time.sleep(0.05)
            raise TimeoutError("jobs did not drain before the timeout")
        finally:
            self._shutdown(30.0)

    def _shutdown(self, grace_seconds: float) -> None:
        deadline = time.monotonic() + grace_seconds
        while self._running and time.monotonic() < deadline:
            self._reap()
            time.sleep(0.1)
        self._executor.shutdown(wait=False, cancel_futures=True)
        self.app.queue.stop_worker(self.worker_id)
        log.info(
            "worker stopped",
            extra={"worker": self.worker_id, "completed": self.completed, "failed": self.failed},
        )

    def _startup_recovery(self) -> None:
        from workgraph.core.context import RequestContext

        try:
            self.app.queue.recover_expired()
            self.app.worktrees.recover(RequestContext.system("worker"))
        except Exception:  # recovery problems must not prevent the worker from starting
            log.exception("startup recovery failed")

    # ------------------------------------------------------------------ jobs

    def _start(self, job: ClaimedJob) -> None:
        ctx = JobContext(job=job, worker_id=self.worker_id)
        future = self._executor.submit(self._execute, job, ctx)
        with self._lock:
            self._running[job.id] = future

    def _reap(self) -> None:
        with self._lock:
            done = [jid for jid, fut in self._running.items() if fut.done()]
            for jid in done:
                self._running.pop(jid)

    def _execute(self, job: ClaimedJob, ctx: JobContext) -> None:
        queue = self.app.queue
        stop_heartbeat = threading.Event()
        started = time.monotonic()

        def heartbeat() -> None:
            interval = max(1.0, self.lease_seconds / 3)
            while not stop_heartbeat.wait(interval):
                try:
                    if queue.heartbeat(job.id, self.worker_id, self.lease_seconds):
                        ctx._cancel.set()
                    if job.timeout_seconds and time.monotonic() - started > job.timeout_seconds:
                        ctx._cancel.set()
                except ConflictError:
                    ctx._cancel.set()
                    return
                except Exception:
                    log.exception("heartbeat failed", extra={"job": job.id})

        beat = threading.Thread(target=heartbeat, name=f"hb-{job.id[:8]}", daemon=True)
        beat.start()
        handler = self.handlers.get(job.kind)
        try:
            if handler is None:
                queue.fail(
                    job.id,
                    self.worker_id,
                    f"no handler registered for job kind {job.kind!r}",
                    retryable=False,
                    error_kind="unknown_kind",
                )
                self.failed += 1
                return
            result = handler(self.app, ctx)
            if ctx.cancel_requested():
                queue.mark_cancelled(job.id, self.worker_id, "cancellation requested")
            else:
                queue.complete(job.id, self.worker_id, result or {})
                self.completed += 1
        except JobCancelled as exc:
            queue.mark_cancelled(job.id, self.worker_id, str(exc))
        except ProviderUnavailableError as exc:
            self.failed += 1
            queue.fail(job.id, self.worker_id, exc.message, retryable=True, error_kind=exc.code)
        except NON_RETRYABLE as exc:
            self.failed += 1
            queue.fail(job.id, self.worker_id, exc.message, retryable=False, error_kind=exc.code)
        except WorkgraphError as exc:
            self.failed += 1
            queue.fail(job.id, self.worker_id, exc.message, retryable=True, error_kind=exc.code)
        except Exception as exc:
            self.failed += 1
            log.exception("job failed", extra={"job": job.id, "kind": job.kind})
            queue.fail(
                job.id, self.worker_id, f"{type(exc).__name__}: {exc}", retryable=True, error_kind="exception"
            )
        finally:
            stop_heartbeat.set()

    # ------------------------------------------------------------------ maintenance

    def _heartbeat_worker(self) -> None:
        now = time.monotonic()
        if now - self._last_heartbeat < 5:
            return
        self._last_heartbeat = now
        with self._lock:
            current = list(self._running)
        try:
            self.app.queue.worker_heartbeat(self.worker_id, current, self.completed, self.failed)
        except Exception:
            log.exception("worker heartbeat failed")

    def _maybe_maintenance(self, *, force: bool = False) -> None:
        now = time.monotonic()
        if not force and now - self._last_maintenance < self.maintenance_interval:
            return
        self._last_maintenance = now
        from workgraph.jobs.maintenance import run_maintenance

        try:
            run_maintenance(
                self.app, self.worker_id, lease_seconds=max(10, int(self.maintenance_interval * 3))
            )
        except Exception:
            log.exception("maintenance failed")
