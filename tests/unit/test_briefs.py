from __future__ import annotations

from dataclasses import replace

import pytest

from workgraph.core.errors import ValidationFailed
from workgraph.experiments.briefs import BriefContext, render

CONTEXT = BriefContext(
    task_key="WG-1",
    trial_id="T-1",
    repository="demo",
    worktree="/w/demo-wt",
    branch="agent/wg-1",
    subject="Add g",
    body="Return g.",
    test_paths=("tests/test_mod.py",),
    oracle=(("python", "-m", "pytest", "-q", "tests/test_mod.py"),),
    gates=(("uv", "run", "ruff", "check", "."),),
)

GOLDEN_REPLAY_V1 = """\
Task-ID: WG-1
Trial-ID: T-1

You are working alone in the git worktree /w/demo-wt (repository demo), \
on branch agent/wg-1. Work only there. Do not push or merge.

## Task
Add g

Return g.

## Tests that must pass
These test files were added or changed for this task and fail now:
- tests/test_mod.py

Make them pass by changing the code, not the tests. Do not modify, delete or skip these test files.

Run them with:
    python -m pytest -q tests/test_mod.py

## Checks that must also pass
    uv run ruff check .

## Rules
Follow the repository's own agent instructions (AGENTS.md or CLAUDE.md) if it has them. Commit your work on agent/wg-1 \
when you are done.

## Report
Reply with what you changed, the output of the test command, and anything you could not finish.
"""


def test_replay_v1_matches_golden_text() -> None:
    assert render("replay-v1", CONTEXT) == GOLDEN_REPLAY_V1


def test_no_gates_renders_none() -> None:
    rendered = render("replay-v1", replace(CONTEXT, gates=()))

    assert "## Checks that must also pass\n    (none)\n\n## Rules\n" in rendered
    assert "ruff" not in rendered


def test_empty_body_is_followed_directly_by_next_section() -> None:
    rendered = render("replay-v1", replace(CONTEXT, body=""))

    assert "## Task\nAdd g\n\n## Tests that must pass\n" in rendered
    assert "\n\n\n" not in rendered


def test_unknown_template_is_rejected_naming_known_ids() -> None:
    with pytest.raises(ValidationFailed, match="replay-v1"):
        render("replay-v9", CONTEXT)


def test_rendered_output_ends_with_exactly_one_newline() -> None:
    rendered = render("replay-v1", CONTEXT)

    assert rendered.endswith("anything you could not finish.\n")
    assert not rendered.endswith("\n\n")
