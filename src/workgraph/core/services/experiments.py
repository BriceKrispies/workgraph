"""Experiments: a pinned protocol and the trials measured under it.

An experiment is drafted, frozen (its protocol becomes immutable and it starts running), and closed.
Trials can only change while their experiment is running. Every trial is one task, one attempt, one
model and one arm; the model is the arm's pinned id, and a trial that ran a different model is
invalid rather than silently counted.
"""

from __future__ import annotations

import builtins
from enum import StrEnum
from typing import Any

from sqlalchemy import func, select
from sqlalchemy.orm import Session

from workgraph.core.clock import is_uuid, utcnow
from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, NotFoundError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import (
    ExperimentStatus,
    FailureCause,
    FailureCauseSource,
    TrialOutcome,
    TrialPhase,
    TrialStatus,
)
from workgraph.core.models.schemas import SLUG_RE, ExperimentOut, TrialOut
from workgraph.core.policies.experiments import validate_protocol
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.persistence.orm import Experiment, ExperimentTrial, SubagentRun, Task
from workgraph.security.permissions import Permission, require


def _choice[E: StrEnum](value: str, enum_cls: type[E], what: str) -> E:
    try:
        return enum_cls(value)
    except ValueError:
        allowed = [member.value for member in enum_cls]
        raise ValidationFailed(
            f"{what} {value!r} is not one of: {', '.join(allowed)}", details={"allowed": allowed}
        ) from None


def experiment_out(experiment: Experiment) -> ExperimentOut:
    return ExperimentOut(
        id=experiment.id,
        workspace_id=experiment.workspace_id,
        slug=experiment.slug,
        title=experiment.title,
        question=experiment.question,
        status=experiment.status,
        protocol=experiment.protocol,
        frozen_at=experiment.frozen_at,
        created_at=experiment.created_at,
        updated_at=experiment.updated_at,
    )


def trial_out(session: Session, trial: ExperimentTrial) -> TrialOut:
    task = session.get(Task, trial.task_id)
    assert task is not None
    return TrialOut(
        id=trial.id,
        experiment_id=trial.experiment_id,
        task_id=trial.task_id,
        task_key=task.key,
        attempt_id=trial.attempt_id,
        subagent_run_id=trial.subagent_run_id,
        arm=trial.arm,
        requested_model=trial.requested_model,
        actual_model=trial.actual_model,
        phase=trial.phase,
        seq=trial.seq,
        band=trial.band,
        features=trial.features,
        base_ref=trial.base_ref,
        worktree_path=trial.worktree_path,
        status=trial.status,
        outcome=trial.outcome,
        outcome_detail=trial.outcome_detail,
        failure_cause=trial.failure_cause,
        failure_cause_source=trial.failure_cause_source,
        notes=trial.notes,
        created_at=trial.created_at,
        started_at=trial.started_at,
        graded_at=trial.graded_at,
    )


class ExperimentService(Service):
    # ------------------------------------------------------------------ internal helpers

    @staticmethod
    def _find(session: Session, workspace_id: str, ref: str) -> Experiment:
        if is_uuid(ref):
            experiment = session.get(Experiment, ref)
        else:
            experiment = session.scalar(
                select(Experiment).where(Experiment.workspace_id == workspace_id, Experiment.slug == ref)
            )
        if experiment is None or experiment.workspace_id != workspace_id:
            raise NotFoundError(f"experiment {ref!r} not found", details={"ref": ref})
        return experiment

    @staticmethod
    def _trial(session: Session, workspace_id: str, ref: str) -> ExperimentTrial:
        trial = session.get(ExperimentTrial, ref) if is_uuid(ref) else None
        if trial is None:
            raise NotFoundError(f"trial {ref!r} not found", details={"ref": ref})
        experiment = session.get(Experiment, trial.experiment_id)
        if experiment is None or experiment.workspace_id != workspace_id:
            raise NotFoundError(f"trial {ref!r} not found", details={"ref": ref})
        return trial

    @staticmethod
    def _require_running(experiment: Experiment) -> None:
        if experiment.status != ExperimentStatus.RUNNING:
            raise ConflictError(
                f"experiment {experiment.slug} is {experiment.status}; trials change only while it is running",
                details={"experiment": experiment.slug, "status": experiment.status},
            )

    @staticmethod
    def _arms(experiment: Experiment) -> dict[str, str]:
        return {arm["name"]: arm["model"] for arm in experiment.protocol["arms"]}

    def _link(
        self,
        session: Session,
        ctx: RequestContext,
        trial: ExperimentTrial,
        *,
        attempt_id: str | None,
        subagent_run_id: str | None,
        actual_model: str | None,
    ) -> None:
        experiment = session.get(Experiment, trial.experiment_id)
        assert experiment is not None
        self._require_running(experiment)
        if attempt_id is not None:
            trial.attempt_id = attempt_id
        if subagent_run_id is not None:
            trial.subagent_run_id = subagent_run_id
        if actual_model is not None:
            trial.actual_model = actual_model
        session.flush()
        linked = {
            "experiment": experiment.slug,
            "attempt_id": attempt_id,
            "subagent_run_id": subagent_run_id,
            "actual_model": actual_model,
            "requested_model": trial.requested_model,
        }
        record_event(
            session,
            ctx,
            event_type="experiment_trial.linked",
            entity_type="experiment_trial",
            entity_id=trial.id,
            workspace_id=experiment.workspace_id,
            data=linked,
        )
        if (
            actual_model is not None
            and actual_model != trial.requested_model
            and trial.status != TrialStatus.INVALID
        ):
            # The arm pins one model; a run on another one measures a different thing and stays out of fits.
            trial.status = str(TrialStatus.INVALID)
            trial.failure_cause = str(FailureCause.WRONG_MODEL)
            trial.failure_cause_source = str(FailureCauseSource.MECHANICAL)
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment_trial.invalidated",
                entity_type="experiment_trial",
                entity_id=trial.id,
                workspace_id=experiment.workspace_id,
                data={
                    "experiment": experiment.slug,
                    "failure_cause": str(FailureCause.WRONG_MODEL),
                    "failure_cause_source": str(FailureCauseSource.MECHANICAL),
                    "requested_model": trial.requested_model,
                    "actual_model": actual_model,
                },
            )

    # ------------------------------------------------------------------ reads

    def list(
        self, ctx: RequestContext, workspace: str | None, *, status: str | None = None
    ) -> builtins.list[ExperimentOut]:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            stmt = select(Experiment).where(Experiment.workspace_id == ws_id).order_by(Experiment.created_at)
            if status is not None:
                stmt = stmt.where(Experiment.status == _choice(status, ExperimentStatus, "status"))
            return [experiment_out(experiment) for experiment in session.scalars(stmt)]

    def get(self, ctx: RequestContext, workspace: str | None, ref: str) -> ExperimentOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            return experiment_out(self._find(session, ws_id, ref))

    def trials(self, ctx: RequestContext, workspace: str | None, ref: str) -> builtins.list[TrialOut]:
        """An experiment's trials in the order they were added."""
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            experiment = self._find(session, ws_id, ref)
            rows = session.scalars(
                select(ExperimentTrial)
                .where(ExperimentTrial.experiment_id == experiment.id)
                .order_by(ExperimentTrial.seq)
            )
            return [trial_out(session, trial) for trial in rows]

    def trial(self, ctx: RequestContext, workspace: str | None, ref: str) -> TrialOut:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            return trial_out(session, self._trial(session, ws_id, ref))

    # ------------------------------------------------------------------ experiment mutations

    def create(
        self,
        ctx: RequestContext,
        workspace: str | None,
        slug: str,
        title: str,
        question: str | None,
        protocol: dict[str, Any],
    ) -> ExperimentOut:
        require(ctx, Permission.EXPERIMENT_WRITE)
        if not SLUG_RE.match(slug):
            raise ValidationFailed(f"experiment slug {slug!r} must be lowercase letters, digits and hyphens")
        if not title.strip():
            raise ValidationFailed("experiment title must not be empty")
        validate_protocol(protocol)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            exists = session.scalar(
                select(Experiment.id).where(Experiment.workspace_id == ws_id, Experiment.slug == slug)
            )
            if exists is not None:
                raise ConflictError(
                    f"experiment {slug!r} already exists in this workspace", details={"slug": slug}
                )
            experiment = Experiment(
                workspace_id=ws_id,
                slug=slug,
                title=title,
                question=question,
                status=str(ExperimentStatus.DRAFT),
                protocol=protocol,
            )
            session.add(experiment)
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment.created",
                entity_type="experiment",
                entity_id=experiment.id,
                workspace_id=ws_id,
                data={"slug": slug, "arms": sorted(self._arms(experiment))},
            )
            return experiment_out(experiment)

    def update_protocol(
        self, ctx: RequestContext, workspace: str | None, ref: str, protocol: dict[str, Any]
    ) -> ExperimentOut:
        require(ctx, Permission.EXPERIMENT_WRITE)
        validate_protocol(protocol)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            experiment = self._find(session, ws_id, ref)
            if experiment.status != ExperimentStatus.DRAFT:
                raise ConflictError(
                    f"the protocol of {experiment.slug} is frozen; it cannot change once the experiment is {experiment.status}",
                    details={"experiment": experiment.slug, "status": experiment.status},
                )
            experiment.protocol = protocol
            experiment.updated_at = utcnow()
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment.protocol_updated",
                entity_type="experiment",
                entity_id=experiment.id,
                workspace_id=ws_id,
                data={"slug": experiment.slug, "protocol": protocol},
            )
            return experiment_out(experiment)

    def freeze(self, ctx: RequestContext, workspace: str | None, ref: str) -> ExperimentOut:
        """Validate the protocol, freeze it and start the experiment."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            experiment = self._find(session, ws_id, ref)
            if experiment.status != ExperimentStatus.DRAFT:
                raise ConflictError(
                    f"{experiment.slug} is already {experiment.status}; only a draft can be frozen",
                    details={"experiment": experiment.slug, "status": experiment.status},
                )
            validate_protocol(experiment.protocol)
            now = utcnow()
            experiment.status = str(ExperimentStatus.RUNNING)
            experiment.frozen_at = now
            experiment.updated_at = now
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment.frozen",
                entity_type="experiment",
                entity_id=experiment.id,
                workspace_id=ws_id,
                data={"slug": experiment.slug, "protocol": experiment.protocol},
            )
            return experiment_out(experiment)

    def close(self, ctx: RequestContext, workspace: str | None, ref: str) -> ExperimentOut:
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            experiment = self._find(session, ws_id, ref)
            if experiment.status != ExperimentStatus.RUNNING:
                raise ConflictError(
                    f"{experiment.slug} is {experiment.status}; only a running experiment can be closed",
                    details={"experiment": experiment.slug, "status": experiment.status},
                )
            experiment.status = str(ExperimentStatus.CLOSED)
            experiment.updated_at = utcnow()
            session.flush()
            trials = session.scalar(
                select(func.count())
                .select_from(ExperimentTrial)
                .where(ExperimentTrial.experiment_id == experiment.id)
            )
            record_event(
                session,
                ctx,
                event_type="experiment.closed",
                entity_type="experiment",
                entity_id=experiment.id,
                workspace_id=ws_id,
                data={"slug": experiment.slug, "trials": trials or 0},
            )
            return experiment_out(experiment)

    # ------------------------------------------------------------------ trial mutations

    def add_trial(
        self,
        ctx: RequestContext,
        workspace: str | None,
        ref: str,
        task_ref: str,
        arm: str,
        phase: str,
        band: int | None = None,
        features: dict[str, Any] | None = None,
    ) -> TrialOut:
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            experiment = self._find(session, ws_id, ref)
            self._require_running(experiment)
            task = resolve.task(session, ws_id, task_ref)
            arms = self._arms(experiment)
            if arm not in arms:
                raise ValidationFailed(
                    f"arm {arm!r} is not in the protocol of {experiment.slug}",
                    details={"arms": sorted(arms)},
                )
            trial_phase = _choice(phase, TrialPhase, "phase")
            bands = experiment.protocol["bands"]
            if band is not None and not bands["min"] <= band <= bands["max"]:
                raise ValidationFailed(
                    f"band {band} is outside the protocol range {bands['min']}..{bands['max']}",
                    details={"band": band, "min": bands["min"], "max": bands["max"]},
                )
            seq = (
                session.scalar(
                    select(func.max(ExperimentTrial.seq)).where(
                        ExperimentTrial.experiment_id == experiment.id
                    )
                )
                or 0
            ) + 1
            trial = ExperimentTrial(
                experiment_id=experiment.id,
                task_id=task.id,
                arm=arm,
                requested_model=arms[arm],
                phase=str(trial_phase),
                seq=seq,
                band=band,
                features={"predicted": features or {}},
                status=str(TrialStatus.PLANNED),
            )
            session.add(trial)
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment_trial.added",
                entity_type="experiment_trial",
                entity_id=trial.id,
                workspace_id=ws_id,
                data={
                    "experiment": experiment.slug,
                    "task_key": task.key,
                    "arm": arm,
                    "phase": str(trial_phase),
                    "seq": seq,
                    "band": band,
                    "requested_model": arms[arm],
                },
            )
            return trial_out(session, trial)

    def start_trial(
        self,
        ctx: RequestContext,
        workspace: str | None,
        trial_ref: str,
        *,
        base_ref: str | None = None,
        worktree_path: str | None = None,
    ) -> TrialOut:
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            trial = self._trial(session, ws_id, trial_ref)
            experiment = session.get(Experiment, trial.experiment_id)
            assert experiment is not None
            self._require_running(experiment)
            if trial.status != TrialStatus.PLANNED:
                raise ConflictError(
                    f"trial {trial.id} is {trial.status}; only a planned trial can start",
                    details={"trial": trial.id, "status": trial.status},
                )
            trial.status = str(TrialStatus.RUNNING)
            trial.started_at = utcnow()
            trial.base_ref = base_ref
            trial.worktree_path = worktree_path
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment_trial.started",
                entity_type="experiment_trial",
                entity_id=trial.id,
                workspace_id=ws_id,
                data={"experiment": experiment.slug, "base_ref": base_ref, "worktree_path": worktree_path},
            )
            return trial_out(session, trial)

    def record_grade(
        self,
        ctx: RequestContext,
        workspace: str | None,
        trial_ref: str,
        outcome: str,
        *,
        outcome_detail: dict[str, Any] | None = None,
        features: dict[str, Any] | None = None,
    ) -> TrialOut:
        """Record the measured outcome. An invalid trial keeps its status but still stores its outcome."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            trial = self._trial(session, ws_id, trial_ref)
            experiment = session.get(Experiment, trial.experiment_id)
            assert experiment is not None
            self._require_running(experiment)
            measured = _choice(outcome, TrialOutcome, "outcome")
            if trial.status not in (TrialStatus.RUNNING, TrialStatus.INVALID):
                raise ConflictError(
                    f"trial {trial.id} is {trial.status}; only a running or invalid trial takes a grade",
                    details={"trial": trial.id, "status": trial.status},
                )
            if trial.status == TrialStatus.RUNNING:
                trial.status = str(TrialStatus.GRADED)
            trial.outcome = str(measured)
            trial.outcome_detail = outcome_detail or {}
            if features is not None:
                trial.features = {**trial.features, "actual": features}
            trial.graded_at = utcnow()
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment_trial.graded",
                entity_type="experiment_trial",
                entity_id=trial.id,
                workspace_id=ws_id,
                data={"experiment": experiment.slug, "outcome": str(measured), "status": trial.status},
            )
            return trial_out(session, trial)

    def classify(
        self,
        ctx: RequestContext,
        workspace: str | None,
        trial_ref: str,
        cause: str,
        source: str,
        notes: str | None = None,
    ) -> TrialOut:
        """Name why a failed or invalid trial failed, and who or what decided it."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            trial = self._trial(session, ws_id, trial_ref)
            experiment = session.get(Experiment, trial.experiment_id)
            assert experiment is not None
            self._require_running(experiment)
            failure = _choice(cause, FailureCause, "failure cause")
            decided_by = _choice(source, FailureCauseSource, "failure cause source")
            failed = trial.status == TrialStatus.GRADED and trial.outcome == TrialOutcome.FAILED
            if not failed and trial.status != TrialStatus.INVALID:
                raise ConflictError(
                    f"trial {trial.id} is {trial.status}; only failed or invalid trials are classified",
                    details={"trial": trial.id, "status": trial.status, "outcome": trial.outcome},
                )
            trial.failure_cause = str(failure)
            trial.failure_cause_source = str(decided_by)
            if notes is not None:
                trial.notes = notes
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment_trial.classified",
                entity_type="experiment_trial",
                entity_id=trial.id,
                workspace_id=ws_id,
                data={
                    "experiment": experiment.slug,
                    "failure_cause": str(failure),
                    "failure_cause_source": str(decided_by),
                },
            )
            return trial_out(session, trial)

    def mark_invalid(
        self,
        ctx: RequestContext,
        workspace: str | None,
        trial_ref: str,
        cause: str,
        notes: str | None = None,
    ) -> TrialOut:
        """Exclude a trial from every fit, recording the reason as a human decision."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            trial = self._trial(session, ws_id, trial_ref)
            experiment = session.get(Experiment, trial.experiment_id)
            assert experiment is not None
            self._require_running(experiment)
            failure = _choice(cause, FailureCause, "failure cause")
            trial.status = str(TrialStatus.INVALID)
            trial.failure_cause = str(failure)
            trial.failure_cause_source = str(FailureCauseSource.HUMAN)
            if notes is not None:
                trial.notes = notes
            session.flush()
            record_event(
                session,
                ctx,
                event_type="experiment_trial.invalidated",
                entity_type="experiment_trial",
                entity_id=trial.id,
                workspace_id=ws_id,
                data={
                    "experiment": experiment.slug,
                    "failure_cause": str(failure),
                    "failure_cause_source": str(FailureCauseSource.HUMAN),
                },
            )
            return trial_out(session, trial)

    def link_run(
        self,
        ctx: RequestContext,
        workspace: str | None,
        trial_ref: str,
        *,
        attempt_id: str | None = None,
        subagent_run_id: str | None = None,
        actual_model: str | None = None,
    ) -> TrialOut:
        """Attach the attempt or subagent run that executed a trial, and the model that actually ran."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            trial = self._trial(session, ws_id, trial_ref)
            self._link(
                session,
                ctx,
                trial,
                attempt_id=attempt_id,
                subagent_run_id=subagent_run_id,
                actual_model=actual_model,
            )
            return trial_out(session, trial)

    def link_from_subagent_run(self, ctx: RequestContext, subagent_run_id: str) -> builtins.list[TrialOut]:
        """Link the trials named by a subagent run's ``Trial-ID`` markers."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        with self.db.session(write=True) as session:
            run = session.get(SubagentRun, subagent_run_id)
            if run is None:
                raise NotFoundError(
                    f"subagent run {subagent_run_id!r} not found", details={"ref": subagent_run_id}
                )
            return self.link_run_markers(session, ctx, run)

    def link_run_markers(self, session: Session, ctx: RequestContext, run: Any) -> builtins.list[TrialOut]:
        """Link each trial a run names in ``telemetry["trial_ids"]``, using the model the run recorded.

        ``telemetry`` and ``model`` are read defensively: they are columns that a parallel change adds to
        ``subagent_runs``, so a run without them names no trials and records no model.
        """
        telemetry = getattr(run, "telemetry", None)
        trial_ids = telemetry.get("trial_ids") if isinstance(telemetry, dict) else None
        if not isinstance(trial_ids, list) or not trial_ids:
            return []
        actual_model = getattr(run, "model", None)
        linked: list[TrialOut] = []
        for trial_id in trial_ids:
            trial = self._trial_for_run(session, trial_id)
            self._link(
                session,
                ctx,
                trial,
                attempt_id=run.attempt_id,
                subagent_run_id=run.id,
                actual_model=actual_model if isinstance(actual_model, str) else None,
            )
            linked.append(trial_out(session, trial))
        return linked

    def _trial_for_run(self, session: Session, trial_id: object) -> ExperimentTrial:
        trial = session.get(ExperimentTrial, trial_id) if isinstance(trial_id, str) else None
        if trial is None:
            raise NotFoundError(
                f"trial {trial_id!r} named by a subagent run was not found", details={"trial": trial_id}
            )
        return trial
