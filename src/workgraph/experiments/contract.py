"""Leaf contract check: decide whether a task can go out as a leaf, must be split, or cannot be delegated.

The decision runs in a fixed order. A task that needs a human decision, has no checkable oracle, or is a
report or register item is ``not_delegable``. A task whose ``metadata.contract`` is missing or malformed is
``incomplete``. Otherwise the predictor scores the declared size: ``leaf`` when predicted success meets the
target, ``split`` when it does not. The predicted success is always an inference from the named fit.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Literal

from workgraph.core.context import RequestContext
from workgraph.core.errors import NotFoundError, ValidationFailed
from workgraph.core.models.schemas import PredictionOut, TaskOut
from workgraph.core.services.base import Service
from workgraph.security.permissions import Permission, require

Verdict = Literal["leaf", "split", "not_delegable", "incomplete"]

NOT_DELEGABLE_TAGS = (
    "needs-decision",
    "kind:report",
    "kind:register-row",
    "kind:split-status",
    "kind:manifest",
)


@dataclass(frozen=True)
class ContractVerdict:
    verdict: Verdict
    reasons: tuple[str, ...]
    missing: tuple[str, ...]
    prediction: PredictionOut | None
    target: float
    model: str

    def as_dict(self) -> dict[str, Any]:
        return {
            "verdict": self.verdict,
            "reasons": list(self.reasons),
            "missing": list(self.missing),
            "prediction": self.prediction.model_dump(mode="json") if self.prediction is not None else None,
            "target": self.target,
            "model": self.model,
        }


def _is_positive(value: object) -> bool:
    return isinstance(value, int | float) and not isinstance(value, bool) and value > 0


def _non_empty_strings(value: object) -> bool:
    return isinstance(value, list) and bool(value) and all(isinstance(v, str) and v.strip() for v in value)


def _argv_lists(value: object, *, allow_empty: bool) -> bool:
    if not isinstance(value, list) or (not value and not allow_empty):
        return False
    return all(
        isinstance(argv, list) and bool(argv) and all(isinstance(part, str) and part for part in argv)
        for argv in value
    )


def _has_oracle(contract: dict[str, Any]) -> bool:
    oracle = contract.get("oracle")
    return isinstance(oracle, list) and bool(oracle)


def contract_problems(task: TaskOut) -> tuple[list[str], list[str]]:
    """``(missing, not_delegable)``: the contract fields absent or malformed, and the reasons it cannot go out."""
    not_delegable = [
        f"tag {tag} makes the task not delegable" for tag in task.tags if tag in NOT_DELEGABLE_TAGS
    ]
    contract_raw = task.metadata.get("contract")
    contract: dict[str, Any] = contract_raw if isinstance(contract_raw, dict) else {}
    if not _has_oracle(contract) and not task.acceptance_criteria:
        not_delegable.append(
            "no checkable oracle: the task has no contract oracle and no acceptance criteria"
        )

    missing: list[str] = []
    if not _non_empty_strings(contract.get("files")):
        missing.append("contract.files")
    if not _argv_lists(contract.get("oracle"), allow_empty=False):
        missing.append("contract.oracle")
    if not _argv_lists(contract.get("gates"), allow_empty=True):
        missing.append("contract.gates")
    if not _non_empty_strings(contract.get("out_of_scope")):
        missing.append("contract.out_of_scope")

    checks = contract.get("checks")
    criteria = len(task.acceptance_criteria)
    if not (
        isinstance(checks, list)
        and all(isinstance(c, str) and c.strip() for c in checks)
        and len(checks) == criteria
    ):
        got = len(checks) if isinstance(checks, list) else "none"
        missing.append(f"contract.checks ({criteria} acceptance criteria, {got} checks, one non-empty each)")

    predicted_raw = contract.get("predicted")
    predicted: dict[str, Any] = predicted_raw if isinstance(predicted_raw, dict) else {}
    for name in ("src_lines", "src_files"):
        value = predicted.get(name)
        if not _is_positive(value):
            missing.append(f"contract.predicted.{name}")
    packages = predicted.get("packages")
    if not isinstance(packages, list) or not all(isinstance(p, str) for p in packages):
        missing.append("contract.predicted.packages")
    return missing, not_delegable


class ContractChecker(Service):
    def check(
        self,
        ctx: RequestContext,
        workspace: str | None,
        task_ref: str,
        *,
        model: str,
        target: float = 0.8,
    ) -> ContractVerdict:
        require(ctx, Permission.TASK_READ)
        if not 0.0 <= target <= 1.0:
            raise ValidationFailed(
                f"target must be between 0 and 1, got {target}", details={"target": target}
            )
        task = self.app.tasks.get(ctx, workspace, task_ref)
        missing, not_delegable = contract_problems(task)
        if not_delegable:
            return ContractVerdict("not_delegable", tuple(not_delegable), (), None, target, model)
        if missing:
            reason = "contract is missing or malformed: fix the fields listed in missing"
            return ContractVerdict("incomplete", (reason,), tuple(missing), None, target, model)

        try:
            prediction = self.app.predictor.predict_task(ctx, workspace, model, task.key)
        except NotFoundError:
            reason = f"no predictor fit for {model}: run workgraph experiment fit --model {model}"
            return ContractVerdict("incomplete", (reason,), (), None, target, model)

        if prediction.probability < target:
            reason = f"predicted success {prediction.probability:.3f} is below the target {target:.3f}: split"
            return ContractVerdict("split", (reason,), (), prediction, target, model)

        reasons = [f"predicted success {prediction.probability:.3f} meets the target {target:.3f}"]
        if not prediction.repository_known:
            reasons.append("repository not seen by the fit: run a replay calibration first")
        return ContractVerdict("leaf", tuple(reasons), (), prediction, target, model)
