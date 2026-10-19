"""Judging a stack's verification run against the stack's base, for checks in regression mode.

A check with ``baseline="absolute"`` fails whenever it does not pass. A check with
``baseline="regression"`` fails only when its failures include one that the base does not already
report: the base is measured by running the same check there, so nothing is written down as an
exception list. A regression-mode check that times out, or that fails without any parsed failure
signature, cannot be compared and counts as failing.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from workgraph.verification.configs import CheckSpec
from workgraph.verification.service import VerificationRunOut


@dataclass(frozen=True)
class CheckOutcome:
    """How one check finished on a revision: its status and its failure signatures."""

    status: str
    signatures: list[str] = field(default_factory=list)
    run_id: str | None = None


def regressions(
    run: VerificationRunOut, base: dict[str, CheckOutcome] | None, specs: dict[str, CheckSpec]
) -> dict[str, list[str]]:
    """The failures of ``run`` that count against it, keyed by check name, as lists of signatures.

    Only required checks can fail a run. A check missing from ``specs`` is judged absolutely.
    """
    found: dict[str, list[str]] = {}
    for check in run.checks:
        if not check.required or check.status == "passed":
            continue
        signatures = list(check.failure_signatures)
        spec = specs.get(check.name)
        if spec is None or spec.baseline == "absolute":
            found[check.name] = signatures
            continue
        if "timeout" in signatures or not signatures:
            found[check.name] = signatures
            continue
        outcome = base.get(check.name) if base else None
        if outcome is None or outcome.status == "passed":
            found[check.name] = signatures
            continue
        known = set(outcome.signatures)
        new = [signature for signature in signatures if signature not in known]
        if new:
            found[check.name] = new
    return found
