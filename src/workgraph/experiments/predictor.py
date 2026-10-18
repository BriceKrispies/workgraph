"""Success predictor: P(success | ex-ante features, model, repository), fitted over graded trials.

The features are the size a leaf is predicted to have (log2 source lines, log2 files) and the number
of packages it touches. Each repository gets its own intercept, shrunk toward the shared fit by an L2
penalty, so a repository with few trials starts near the shared estimate. A fit stores its data cutoff
and the trials it used. Every prediction is an inference that names the fit it came from.
"""

from __future__ import annotations

import math
import random
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime
from typing import Any

from sqlalchemy import select
from sqlalchemy.orm import Session

from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.core.events.log import record_event
from workgraph.core.models.enums import TrialOutcome, TrialStatus
from workgraph.core.models.schemas import PredictionOut, PredictorFitOut, TrialOut
from workgraph.core.services import resolve
from workgraph.core.services.base import Service
from workgraph.experiments.stats import brier, fit_logistic
from workgraph.persistence.orm import PredictorFit
from workgraph.security.permissions import Permission, require

MIN_ROWS = 4
MIN_SAMPLES_FOR_INTERVAL = 20
FEATURE_NAMES = ("log2_src_lines", "log2_src_files", "packages")


@dataclass(frozen=True)
class _Row:
    trial_id: str
    repository: str | None
    x: tuple[float, ...]
    y: int
    graded_at: datetime | None


def _number(value: object) -> float | None:
    if isinstance(value, bool) or not isinstance(value, int | float):
        return None
    return float(value)


def _replay_features(trial: TrialOut) -> dict[str, Any] | None:
    """The replay features a trial was planned with (stored under ``predicted`` by ``add_trial``)."""
    for source in (trial.features.get("predicted"), trial.features):
        if isinstance(source, dict) and isinstance(source.get("replay"), dict):
            return source["replay"]
    return None


def features_of(src_lines: float, src_files: float, packages: int) -> list[float]:
    """The ex-ante feature vector: log2 lines and files (each at least 1), then the package count."""
    return [math.log2(max(1.0, src_lines)), math.log2(max(1.0, src_files)), float(packages)]


def _sigmoid(eta: float) -> float:
    if eta >= 0:
        return 1.0 / (1.0 + math.exp(-eta))
    e = math.exp(eta)
    return e / (1.0 + e)


def _probability(coef: Sequence[float], x: Sequence[float]) -> float:
    """``coef`` is intercept first; ``x`` holds the features without an intercept column."""
    return _sigmoid(coef[0] + sum(c * v for c, v in zip(coef[1:], x, strict=True)))


def _percentile(sorted_values: Sequence[float], q: float) -> float:
    pos = q * (len(sorted_values) - 1)
    low = math.floor(pos)
    high = min(low + 1, len(sorted_values) - 1)
    frac = pos - low
    return sorted_values[low] * (1.0 - frac) + sorted_values[high] * frac


def _latest_row(session: Session, ws_id: str, model: str) -> PredictorFit | None:
    return session.scalar(
        select(PredictorFit)
        .where(PredictorFit.workspace_id == ws_id, PredictorFit.model == model)
        .order_by(PredictorFit.created_at.desc())
        .limit(1)
    )


class Predictor(Service):
    # ------------------------------------------------------------------ data

    def _rows(self, ctx: RequestContext, workspace: str | None, model: str) -> list[_Row]:
        """Graded trials of ``model`` (actual model, else requested) that carry ex-ante features."""
        rows: list[_Row] = []
        for experiment in self.app.experiments.list(ctx, workspace):
            for trial in self.app.experiments.trials(ctx, workspace, experiment.id):
                if trial.status != TrialStatus.GRADED or trial.outcome is None:
                    continue
                if (trial.actual_model or trial.requested_model) != model:
                    continue
                replay = _replay_features(trial)
                if replay is None:
                    continue
                src_lines = _number(replay.get("src_lines"))
                src_files = _number(replay.get("src_files"))
                packages = replay.get("packages")
                if src_lines is None or src_files is None or not isinstance(packages, list):
                    continue
                task = self.app.tasks.get(ctx, workspace, trial.task_id)
                rows.append(
                    _Row(
                        trial_id=trial.id,
                        repository=task.repository_ids[0] if task.repository_ids else None,
                        x=tuple(features_of(src_lines, src_files, len(packages))),
                        y=1 if trial.outcome == TrialOutcome.PASSED else 0,
                        graded_at=trial.graded_at,
                    )
                )
        return rows

    @staticmethod
    def _design(rows: Sequence[_Row], repositories: Sequence[str]) -> list[list[float]]:
        """Feature rows followed by one 0/1 indicator per repository (rows without a repository get none)."""
        index = {repo: i for i, repo in enumerate(repositories)}
        matrix: list[list[float]] = []
        for row in rows:
            indicators = [0.0] * len(repositories)
            if row.repository in index:
                indicators[index[row.repository]] = 1.0
            matrix.append([*row.x, *indicators])
        return matrix

    # ------------------------------------------------------------------ fit

    def fit(
        self,
        ctx: RequestContext,
        workspace: str | None,
        model: str,
        *,
        repo_penalty: float = 1.0,
        feature_penalty: float = 0.01,
        samples: int = 200,
        seed: int = 0,
    ) -> PredictorFitOut:
        """Fit the predictor for ``model`` over all graded trials, bootstrap it, and store the fit."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        if repo_penalty < 0 or feature_penalty < 0:
            raise ValidationFailed("penalties must be >= 0")
        if samples < 0:
            raise ValidationFailed(f"samples must be >= 0, got {samples}")
        rows = self._rows(ctx, workspace, model)
        if len(rows) < MIN_ROWS:
            raise ValidationFailed(
                f"the predictor for {model} needs at least {MIN_ROWS} graded trials with src_lines, "
                f"src_files and packages; has {len(rows)}",
                details={"model": model, "rows": len(rows)},
            )
        passes = sum(row.y for row in rows)
        if passes in (0, len(rows)):
            raise ValidationFailed(
                f"the predictor for {model} needs both pass and fail outcomes; "
                f"has only {'passes' if passes else 'fails'}",
                details={"model": model, "rows": len(rows), "passes": passes},
            )

        repositories = sorted({row.repository for row in rows if row.repository is not None})
        matrix = self._design(rows, repositories)
        outcomes = [row.y for row in rows]
        # Intercept unpenalised, features and repository intercepts shrunk toward zero.
        penalty = [0.0, *([feature_penalty] * len(FEATURE_NAMES)), *([repo_penalty] * len(repositories))]
        fitted = fit_logistic(matrix, outcomes, penalty=penalty)

        rng = random.Random(seed)
        draws: list[list[float]] = []
        for _ in range(samples):
            picked = rng.choices(range(len(rows)), k=len(rows))
            refit = fit_logistic([matrix[i] for i in picked], [outcomes[i] for i in picked], penalty=penalty)
            if refit.converged:
                draws.append(list(refit.coef))

        probs = [_probability(fitted.coef, x) for x in matrix]
        cutoffs = [row.graded_at for row in rows if row.graded_at is not None]
        with self.db.session(write=True) as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            record = PredictorFit(
                workspace_id=ws_id,
                model=model,
                data_cutoff=max(cutoffs) if cutoffs else None,
                trial_ids=[row.trial_id for row in rows],
                feature_names=[*FEATURE_NAMES, *(f"repository:{repo}" for repo in repositories)],
                repositories=repositories,
                coef=list(fitted.coef),
                penalty=list(fitted.penalty),
                n=len(rows),
                converged=fitted.converged,
                metrics={"brier": brier(probs, outcomes), "n_pass": passes, "n_fail": len(rows) - passes},
                samples=draws,
            )
            session.add(record)
            session.flush()
            record_event(
                session,
                ctx,
                event_type="predictor.fitted",
                entity_type="predictor_fit",
                entity_id=record.id,
                workspace_id=ws_id,
                data={
                    "model": model,
                    "n": record.n,
                    "converged": record.converged,
                    "data_cutoff": record.data_cutoff.isoformat() if record.data_cutoff else None,
                    "repositories": repositories,
                    "samples": len(draws),
                },
            )
            return PredictorFitOut.model_validate(record)

    # ------------------------------------------------------------------ reads

    def latest(self, ctx: RequestContext, workspace: str | None, model: str) -> PredictorFitOut | None:
        require(ctx, Permission.TASK_READ)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            row = _latest_row(session, ws_id, model)
            return PredictorFitOut.model_validate(row) if row is not None else None

    @staticmethod
    def _fit_row(session: Session, ws_id: str, model: str, fit_id: str | None) -> PredictorFit:
        if fit_id is None:
            row = _latest_row(session, ws_id, model)
            if row is None:
                raise NotFoundError(
                    f"no predictor fit for model {model}; create one with "
                    f"`workgraph experiment fit --model {model}`",
                    details={"model": model},
                )
            return row
        row = session.get(PredictorFit, fit_id)
        if row is None or row.workspace_id != ws_id:
            raise NotFoundError(f"predictor fit {fit_id!r} not found", details={"fit": fit_id})
        if row.model != model:
            raise ValidationFailed(
                f"predictor fit {fit_id} is for model {row.model}, not {model}", details={"fit": fit_id}
            )
        return row

    def _repository(self, workspace: str | None, ref: str | None) -> tuple[str | None, str | None]:
        """``(name, id)`` of a repository given by name or id. An unknown reference keeps its text and has no id."""
        if ref is None:
            return None, None
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            try:
                repo = resolve.repository(session, ws_id, ref)
            except NotFoundError:
                return ref, None
            return repo.name, repo.id

    # ------------------------------------------------------------------ predict

    def predict(
        self,
        ctx: RequestContext,
        workspace: str | None,
        model: str,
        repository: str | None,
        src_lines: float,
        src_files: float,
        packages: Sequence[str],
        *,
        fit_id: str | None = None,
    ) -> PredictionOut:
        """Probability of success for a leaf of this size in this repository, with a bootstrap interval."""
        require(ctx, Permission.TASK_READ)
        if src_lines < 0 or src_files < 0:
            raise ValidationFailed("src_lines and src_files must be >= 0")
        repo_name, repo_id = self._repository(workspace, repository)
        with self.db.session() as session:
            ws_id = self.app.tasks._workspace_id(session, workspace)
            row = self._fit_row(session, ws_id, model, fit_id)
            fit_repositories = list(row.repositories)
            coef = list(row.coef)
            draws = [list(draw) for draw in row.samples]
            fit_model, fit_pk, data_cutoff, n = row.model, row.id, row.data_cutoff, row.n

        x = features_of(src_lines, src_files, len(packages))
        x += [1.0 if repo_id is not None and repo == repo_id else 0.0 for repo in fit_repositories]
        probability = _probability(coef, x)
        low: float | None = None
        high: float | None = None
        if len(draws) >= MIN_SAMPLES_FOR_INTERVAL:
            values = sorted(_probability(draw, x) for draw in draws)
            low, high = _percentile(values, 0.025), _percentile(values, 0.975)
        return PredictionOut(
            probability=probability,
            low=low,
            high=high,
            fit_id=fit_pk,
            model=fit_model,
            data_cutoff=data_cutoff,
            n=n,
            repository=repo_name,
            repository_known=repo_id is not None and repo_id in fit_repositories,
            inference=True,
        )

    def predict_task(
        self,
        ctx: RequestContext,
        workspace: str | None,
        model: str,
        task_ref: str,
        *,
        fit_id: str | None = None,
    ) -> PredictionOut:
        """Predict for a task whose metadata ``contract.predicted`` declares src_lines, src_files and packages."""
        require(ctx, Permission.TASK_READ)
        task = self.app.tasks.get(ctx, workspace, task_ref)
        contract = task.metadata.get("contract")
        predicted = contract.get("predicted") if isinstance(contract, dict) else None
        if not isinstance(predicted, dict):
            raise ValidationFailed(
                f"task {task.key} has no metadata contract.predicted (src_lines, src_files, packages)",
                details={"task": task.key},
            )
        for name in ("src_lines", "src_files"):
            value = _number(predicted.get(name))
            if value is None or value < 0:
                raise ValidationFailed(
                    f"task {task.key} is missing contract.predicted.{name} (a non-negative number)",
                    details={"task": task.key, "field": name},
                )
        packages = predicted.get("packages")
        if not isinstance(packages, list):
            raise ValidationFailed(
                f"task {task.key} is missing contract.predicted.packages (a list)",
                details={"task": task.key, "field": "packages"},
            )
        repository = task.repository_ids[0] if task.repository_ids else None
        return self.predict(
            ctx,
            workspace,
            model,
            repository,
            _number(predicted["src_lines"]) or 0.0,
            _number(predicted["src_files"]) or 0.0,
            [str(package) for package in packages],
            fit_id=fit_id,
        )
