from __future__ import annotations

from datetime import UTC, datetime
from typing import Any

from workgraph.core.models.schemas import TaskOut
from workgraph.experiments.contract import contract_problems

NOW = datetime(2026, 10, 18, tzinfo=UTC)


def _contract(**overrides: Any) -> dict[str, Any]:
    contract: dict[str, Any] = {
        "files": ["pkg/mod.py", "tests/test_mod.py"],
        "oracle": [["pytest", "-q", "tests/test_mod.py"]],
        "gates": [["ruff", "check", "."]],
        "out_of_scope": ["the CLI"],
        "checks": ["tests/test_mod.py::test_f"],
        "predicted": {"src_lines": 40, "src_files": 1, "packages": ["pkg"]},
    }
    contract.update(overrides)
    return contract


def _task(
    *,
    tags: list[str] | None = None,
    criteria: list[str] | None = None,
    contract: dict[str, Any] | None = None,
) -> TaskOut:
    return TaskOut(
        id="00000000-0000-0000-0000-000000000001",
        workspace_id="ws",
        key="WG-1",
        number=1,
        title="t",
        description="",
        acceptance_criteria=["the function returns 1"] if criteria is None else criteria,
        status="ready",
        priority="medium",
        category=None,
        tags=tags or [],
        project_ids=[],
        repository_ids=[],
        parent_id=None,
        assignee=None,
        assigned_agent=None,
        claimed_by=None,
        claim_expires_at=None,
        auto_execute=False,
        estimated_effort=None,
        external_refs=[],
        metadata={} if contract is None else {"contract": contract},
        due_at=None,
        scheduled_at=None,
        created_by="tester",
        version=1,
        created_at=NOW,
        updated_at=NOW,
        started_at=None,
        completed_at=None,
        accepted_at=None,
        archived_at=None,
        current_classification_id=None,
    )


def test_complete_contract_has_no_problems():
    assert contract_problems(_task(contract=_contract())) == ([], [])


def test_each_not_delegable_tag_is_named():
    for tag in ("needs-decision", "kind:report", "kind:register-row", "kind:split-status", "kind:manifest"):
        missing, not_delegable = contract_problems(_task(tags=[tag], contract=_contract()))
        assert missing == []
        assert len(not_delegable) == 1 and tag in not_delegable[0]


def test_unrelated_tags_are_delegable():
    assert contract_problems(_task(tags=["capability-frontier"], contract=_contract())) == ([], [])


def test_no_oracle_and_no_criteria_is_not_delegable():
    task = _task(criteria=[], contract=_contract(oracle=[], checks=[]))
    missing, not_delegable = contract_problems(task)
    assert "no checkable oracle" in not_delegable[0]
    assert "contract.oracle" in missing


def test_criteria_without_a_contract_oracle_are_still_checkable():
    missing, not_delegable = contract_problems(_task(contract=_contract(oracle=[])))
    assert not_delegable == []
    assert missing == ["contract.oracle"]


def test_missing_contract_names_every_field():
    missing, not_delegable = contract_problems(_task())
    assert not_delegable == []
    assert missing[:5] == [
        "contract.files",
        "contract.oracle",
        "contract.gates",
        "contract.out_of_scope",
        "contract.checks (1 acceptance criteria, none checks, one non-empty each)",
    ]
    assert missing[5:] == [
        "contract.predicted.src_lines",
        "contract.predicted.src_files",
        "contract.predicted.packages",
    ]


def test_malformed_fields_are_named():
    contract = _contract(
        files=[],
        oracle=[["pytest", ""]],
        gates=[[]],
        out_of_scope=[""],
        predicted={"src_lines": 0, "src_files": True, "packages": "pkg"},
    )
    missing, _ = contract_problems(_task(contract=contract))
    assert missing == [
        "contract.files",
        "contract.oracle",
        "contract.gates",
        "contract.out_of_scope",
        "contract.predicted.src_lines",
        "contract.predicted.src_files",
        "contract.predicted.packages",
    ]


def test_gates_may_be_empty():
    assert contract_problems(_task(contract=_contract(gates=[]))) == ([], [])


def test_packages_may_be_empty():
    contract = _contract(predicted={"src_lines": 4, "src_files": 1, "packages": []})
    assert contract_problems(_task(contract=contract)) == ([], [])


def test_checks_must_match_the_criteria_count():
    task = _task(criteria=["one", "two"], contract=_contract(checks=["tests/a.py::t"]))
    missing, _ = contract_problems(task)
    assert missing == ["contract.checks (2 acceptance criteria, 1 checks, one non-empty each)"]


def test_blank_check_is_malformed():
    task = _task(criteria=["one"], contract=_contract(checks=["  "]))
    missing, _ = contract_problems(task)
    assert missing == ["contract.checks (1 acceptance criteria, 1 checks, one non-empty each)"]
