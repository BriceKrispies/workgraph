"""Durable SQLite-backed job queue.

Guarantees:

* **Atomic claiming** - claims run inside ``BEGIN IMMEDIATE`` so two workers can
  never claim the same job, and resource limits are checked in the same transaction.
* **Leases** - a running job holds a lease that its worker extends via heartbeats.
  Jobs whose lease expires (crashed worker) are recovered and retried.
* **Retries** - exponential backoff up to ``max_attempts``; then the job is moved to
  the dead-letter state (``dead``) and kept for inspection.
* **Idempotency** - an ``idempotency_key`` maps repeated enqueues to one job.
* **Resource limits** - each job declares resource keys (``gpu``, ``agent:<name>``,
  ``repo:<id>``, ``run``); a key's limit bounds how many running jobs may hold it.

Transactions here are short; handlers execute outside of any transaction.
"""

from __future__ import annotations

import builtins
from collections import Counter
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Any

from sqlalchemy import delete, func, or_, select, update
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session

from workgraph.core.clock import new_id, utcnow
from workgraph.core.errors import ConflictError, NotFoundError
from workgraph.core.models.enums import JobStatus
from workgraph.persistence.db import Database
from workgraph.persistence.orm import Job, JobResourceClaim, JobRun, Worker

ACTIVE_STATUSES = (JobStatus.QUEUED, JobStatus.RUNNING)
LimitResolver = Callable[[str], int | None]


@dataclass(frozen=True)
class ClaimedJob:
    id: str
    kind: str
    payload: dict[str, Any]
    attempt: int
    max_attempts: int
    workspace_id: str | None
    task_id: str | None
    attempt_id: str | None
    correlation_id: str | None
    causation_id: str | None
    timeout_seconds: float | None
    lease_expires_at: datetime


def enqueue(
    session: Session,
    kind: str,
    payload: dict[str, Any] | None = None,
    *,
    workspace_id: str | None = None,
    priority: int = 100,
    run_at: datetime | None = None,
    max_attempts: int = 5,
    backoff_base_seconds: float = 2.0,
    timeout_seconds: float | None = None,
    idempotency_key: str | None = None,
    resource_keys: Sequence[str] = (),
    correlation_id: str | None = None,
    causation_id: str | None = None,
    task_id: str | None = None,
    attempt_id: str | None = None,
    created_by: str = "system",
) -> Job:
    """Insert a job inside the caller's transaction (deduplicated by idempotency key)."""
    if idempotency_key is not None:
        existing = session.scalar(select(Job).where(Job.idempotency_key == idempotency_key))
        if existing is not None:
            return existing
    job = Job(
        id=new_id(),
        workspace_id=workspace_id,
        kind=kind,
        payload=payload or {},
        priority=priority,
        run_at=run_at or utcnow(),
        max_attempts=max_attempts,
        backoff_base_seconds=backoff_base_seconds,
        timeout_seconds=timeout_seconds,
        idempotency_key=idempotency_key,
        resource_keys=sorted(set(resource_keys)),
        correlation_id=correlation_id,
        causation_id=causation_id,
        task_id=task_id,
        attempt_id=attempt_id,
        created_by=created_by,
    )
    nested = session.begin_nested()
    try:
        session.add(job)
        session.flush()
        nested.commit()
    except IntegrityError:
        nested.rollback()
        existing = session.scalar(select(Job).where(Job.idempotency_key == idempotency_key))
        if existing is None:
            raise
        return existing
    return job


def backoff_delay(base_seconds: float, attempt: int, cap_seconds: float = 3600.0) -> float:
    """Exponential backoff: base * 2^(attempt-1), capped."""
    return min(cap_seconds, base_seconds * (2 ** max(0, attempt - 1)))


class JobQueue:
    def __init__(self, db: Database, limit_for: LimitResolver | None = None) -> None:
        self.db = db
        self.limit_for = limit_for or (lambda _key: None)

    # ------------------------------------------------------------------ producers

    def enqueue(self, kind: str, payload: dict[str, Any] | None = None, **kwargs: Any) -> str:
        with self.db.session(write=True) as session:
            return enqueue(session, kind, payload, **kwargs).id

    # ------------------------------------------------------------------ claiming

    def claim(
        self,
        worker_id: str,
        *,
        kinds: Sequence[str] | None = None,
        lease_seconds: int = 60,
        now: datetime | None = None,
    ) -> ClaimedJob | None:
        now = now or utcnow()
        with self.db.session(write=True) as session:
            stmt = (
                select(Job)
                .where(Job.status == JobStatus.QUEUED, Job.run_at <= now, Job.cancel_requested.is_(False))
                .order_by(Job.priority.asc(), Job.run_at.asc(), Job.created_at.asc())
                .limit(100)
            )
            if kinds:
                stmt = stmt.where(Job.kind.in_(list(kinds)))
            candidates = list(session.scalars(stmt))
            if not candidates:
                return None
            held = Counter(
                session.scalars(
                    select(JobResourceClaim.resource_key)
                    .join(Job, Job.id == JobResourceClaim.job_id)
                    .where(Job.status == JobStatus.RUNNING)
                )
            )
            chosen: Job | None = None
            for job in candidates:
                if all(self._has_capacity(key, held) for key in job.resource_keys):
                    chosen = job
                    break
            if chosen is None:
                return None
            chosen.status = JobStatus.RUNNING
            chosen.attempts += 1
            chosen.lease_owner = worker_id
            lease_expires_at = now + timedelta(seconds=lease_seconds)
            chosen.lease_expires_at = lease_expires_at
            chosen.heartbeat_at = now
            chosen.started_at = now
            for key in chosen.resource_keys:
                session.add(JobResourceClaim(job_id=chosen.id, resource_key=key))
            session.add(JobRun(job_id=chosen.id, attempt_number=chosen.attempts, worker_id=worker_id))
            session.flush()
            return ClaimedJob(
                id=chosen.id,
                kind=chosen.kind,
                payload=dict(chosen.payload or {}),
                attempt=chosen.attempts,
                max_attempts=chosen.max_attempts,
                workspace_id=chosen.workspace_id,
                task_id=chosen.task_id,
                attempt_id=chosen.attempt_id,
                correlation_id=chosen.correlation_id,
                causation_id=chosen.causation_id,
                timeout_seconds=chosen.timeout_seconds,
                lease_expires_at=lease_expires_at,
            )

    def claim_peek(self, *, kinds: Sequence[str] | None = None) -> str | None:
        """Id of a currently runnable job, without claiming it."""
        with self.db.session() as session:
            stmt = select(Job.id).where(
                Job.status == JobStatus.QUEUED, Job.run_at <= utcnow(), Job.cancel_requested.is_(False)
            )
            if kinds:
                stmt = stmt.where(Job.kind.in_(list(kinds)))
            return session.scalar(stmt.limit(1))

    def _has_capacity(self, key: str, held: Counter[str]) -> bool:
        limit = self.limit_for(key)
        return limit is None or held[key] < limit

    # ------------------------------------------------------------------ worker callbacks

    def heartbeat(self, job_id: str, worker_id: str, lease_seconds: int = 60) -> bool:
        """Extend the lease. Returns ``True`` when cancellation has been requested."""
        now = utcnow()
        with self.db.session(write=True) as session:
            job = session.get(Job, job_id)
            if job is None or job.status != JobStatus.RUNNING or job.lease_owner != worker_id:
                raise ConflictError(f"job {job_id} is no longer leased by {worker_id}")
            job.heartbeat_at = now
            job.lease_expires_at = now + timedelta(seconds=lease_seconds)
            return bool(job.cancel_requested)

    def complete(self, job_id: str, worker_id: str, result: dict[str, Any] | None = None) -> None:
        now = utcnow()
        with self.db.session(write=True) as session:
            job = self._owned(session, job_id, worker_id)
            job.status = JobStatus.SUCCEEDED
            job.result = result or {}
            job.finished_at = now
            job.lease_owner = None
            job.lease_expires_at = None
            self._release(session, job.id)
            self._finish_run(session, job, JobStatus.SUCCEEDED, None)

    def fail(
        self,
        job_id: str,
        worker_id: str,
        error: str,
        *,
        retryable: bool = True,
        error_kind: str | None = None,
    ) -> JobStatus:
        now = utcnow()
        with self.db.session(write=True) as session:
            job = self._owned(session, job_id, worker_id)
            return self._record_failure(
                session, job, error, retryable=retryable, error_kind=error_kind, now=now
            )

    def mark_cancelled(self, job_id: str, worker_id: str, reason: str = "cancelled") -> None:
        with self.db.session(write=True) as session:
            job = self._owned(session, job_id, worker_id)
            job.status = JobStatus.CANCELLED
            job.finished_at = utcnow()
            job.last_error = reason
            job.lease_owner = None
            job.lease_expires_at = None
            self._release(session, job.id)
            self._finish_run(session, job, JobStatus.CANCELLED, reason)

    def _record_failure(
        self,
        session: Session,
        job: Job,
        error: str,
        *,
        retryable: bool,
        error_kind: str | None,
        now: datetime,
    ) -> JobStatus:
        job.last_error = error[:20_000]
        job.error_kind = error_kind
        job.lease_owner = None
        job.lease_expires_at = None
        self._release(session, job.id)
        if retryable and job.attempts < job.max_attempts and not job.cancel_requested:
            job.status = JobStatus.QUEUED
            job.run_at = now + timedelta(seconds=backoff_delay(job.backoff_base_seconds, job.attempts))
            self._finish_run(session, job, JobStatus.FAILED, error)
        else:
            job.status = JobStatus.CANCELLED if job.cancel_requested else JobStatus.DEAD
            job.finished_at = now
            self._finish_run(session, job, JobStatus(job.status), error)
        return JobStatus(job.status)

    def _owned(self, session: Session, job_id: str, worker_id: str) -> Job:
        job = session.get(Job, job_id)
        if job is None:
            raise NotFoundError(f"job {job_id} not found")
        if job.status != JobStatus.RUNNING or job.lease_owner != worker_id:
            raise ConflictError(f"job {job_id} is not leased by {worker_id} (status={job.status})")
        return job

    @staticmethod
    def _release(session: Session, job_id: str) -> None:
        session.execute(delete(JobResourceClaim).where(JobResourceClaim.job_id == job_id))

    @staticmethod
    def _finish_run(session: Session, job: Job, status: JobStatus, error: str | None) -> None:
        run = session.scalar(
            select(JobRun)
            .where(JobRun.job_id == job.id, JobRun.attempt_number == job.attempts)
            .order_by(JobRun.started_at.desc())
            .limit(1)
        )
        if run is not None:
            run.status = status
            run.error = error[:5000] if error else None
            run.finished_at = utcnow()

    # ------------------------------------------------------------------ recovery & control

    def recover_expired(self, now: datetime | None = None) -> builtins.list[str]:
        """Re-queue (or dead-letter) jobs whose worker stopped heartbeating."""
        now = now or utcnow()
        recovered: builtins.list[str] = []
        with self.db.session(write=True) as session:
            expired = list(
                session.scalars(
                    select(Job).where(Job.status == JobStatus.RUNNING, Job.lease_expires_at < now)
                )
            )
            for job in expired:
                self._record_failure(
                    session,
                    job,
                    f"lease expired (worker {job.lease_owner} stopped heartbeating)",
                    retryable=True,
                    error_kind="lease_expired",
                    now=now,
                )
                recovered.append(job.id)
        return recovered

    def cancel(self, job_id: str) -> JobStatus:
        with self.db.session(write=True) as session:
            job = session.get(Job, job_id)
            if job is None:
                raise NotFoundError(f"job {job_id} not found")
            if job.status == JobStatus.QUEUED:
                job.status = JobStatus.CANCELLED
                job.finished_at = utcnow()
                job.cancel_requested = True
            elif job.status == JobStatus.RUNNING:
                job.cancel_requested = True
            return JobStatus(job.status)

    def retry_dead(self, job_id: str) -> None:
        """Manually resurrect a dead-lettered or cancelled job with a fresh retry budget."""
        with self.db.session(write=True) as session:
            job = session.get(Job, job_id)
            if job is None:
                raise NotFoundError(f"job {job_id} not found")
            if job.status not in (JobStatus.DEAD, JobStatus.CANCELLED, JobStatus.FAILED):
                raise ConflictError(f"job {job_id} is {job.status}; only dead/cancelled jobs can be retried")
            job.status = JobStatus.QUEUED
            job.max_attempts = job.attempts + max(1, job.max_attempts)
            job.run_at = utcnow()
            job.cancel_requested = False
            job.finished_at = None

    def get(self, job_id: str) -> Job:
        with self.db.session() as session:
            job = session.get(Job, job_id)
            if job is None:
                raise NotFoundError(f"job {job_id} not found")
            session.expunge(job)
            return job

    def list(
        self,
        *,
        statuses: Sequence[str] | None = None,
        kind: str | None = None,
        workspace_id: str | None = None,
        task_id: str | None = None,
        limit: int = 100,
        offset: int = 0,
    ) -> tuple[builtins.list[Job], int]:
        with self.db.session() as session:
            stmt = select(Job)
            if statuses:
                stmt = stmt.where(Job.status.in_(list(statuses)))
            if kind:
                stmt = stmt.where(Job.kind == kind)
            if workspace_id:
                stmt = stmt.where(or_(Job.workspace_id == workspace_id, Job.workspace_id.is_(None)))
            if task_id:
                stmt = stmt.where(Job.task_id == task_id)
            total = int(session.scalar(select(func.count()).select_from(stmt.subquery())) or 0)
            jobs = list(session.scalars(stmt.order_by(Job.created_at.desc()).limit(limit).offset(offset)))
            for job in jobs:
                session.expunge(job)
            return jobs, total

    def runs(self, job_id: str) -> builtins.list[JobRun]:
        with self.db.session() as session:
            runs = list(
                session.scalars(select(JobRun).where(JobRun.job_id == job_id).order_by(JobRun.started_at))
            )
            for run in runs:
                session.expunge(run)
            return runs

    def stats(self, workspace_id: str | None = None) -> dict[str, int]:
        with self.db.session() as session:
            stmt = select(Job.status, func.count()).group_by(Job.status)
            if workspace_id:
                stmt = stmt.where(or_(Job.workspace_id == workspace_id, Job.workspace_id.is_(None)))
            return {str(status): int(count) for status, count in session.execute(stmt)}

    # ------------------------------------------------------------------ workers

    def register_worker(
        self, worker_id: str, hostname: str, pid: int, concurrency: int, kinds: builtins.list[str]
    ) -> None:
        with self.db.session(write=True) as session:
            worker = session.get(Worker, worker_id)
            if worker is None:
                session.add(
                    Worker(id=worker_id, hostname=hostname, pid=pid, concurrency=concurrency, kinds=kinds)
                )
            else:
                worker.status = "running"
                worker.stopped_at = None
                worker.heartbeat_at = utcnow()

    def worker_heartbeat(
        self, worker_id: str, current_jobs: builtins.list[str], completed: int, failed: int
    ) -> None:
        with self.db.session(write=True) as session:
            session.execute(
                update(Worker)
                .where(Worker.id == worker_id)
                .values(
                    heartbeat_at=utcnow(),
                    current_jobs=current_jobs,
                    jobs_completed=completed,
                    jobs_failed=failed,
                )
            )

    def stop_worker(self, worker_id: str) -> None:
        with self.db.session(write=True) as session:
            session.execute(
                update(Worker)
                .where(Worker.id == worker_id)
                .values(status="stopped", stopped_at=utcnow(), current_jobs=[])
            )

    def mark_lost_workers(self, stale_after_seconds: int = 120) -> builtins.list[str]:
        cutoff = utcnow() - timedelta(seconds=stale_after_seconds)
        with self.db.session(write=True) as session:
            lost = list(
                session.scalars(
                    select(Worker).where(Worker.status == "running", Worker.heartbeat_at < cutoff)
                )
            )
            for worker in lost:
                worker.status = "lost"
            return [w.id for w in lost]

    def workers(self) -> builtins.list[Worker]:
        with self.db.session() as session:
            rows = list(session.scalars(select(Worker).order_by(Worker.started_at.desc()).limit(50)))
            for row in rows:
                session.expunge(row)
            return rows
