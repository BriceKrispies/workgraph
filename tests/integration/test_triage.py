from __future__ import annotations

import json
from pathlib import Path

import pytest
from typer.testing import CliRunner

from workgraph.cli.common import STATE
from workgraph.cli.main import app as cli
from workgraph.core.models.schemas import ClaimRequest, TaskCreate
from workgraph.core.services import triage


def _task(app, human, ws, title, *, tags=(), priority="high", status="backlog"):
    out = app.tasks.create(
        human,
        ws,
        TaskCreate(title=title, tags=list(tags), priority=priority, status=status),
    )
    return out.key


def _accepted(app, human, ws, title):
    key = _task(app, human, ws, title, status="ready")
    app.tasks.claim(human, ws, key, ClaimRequest())
    app.tasks.complete(human, ws, key, summary="done")
    app.verification.accept_manually(human, ws, key, reason="reviewed")
    return key


def _apply(app, human, ws, entries, *, dry_run=False, source="triage batch TEST"):
    return triage.apply_verdicts(app.tasks, human, ws, entries, dry_run=dry_run, source=source)


def _detail(app, human, ws, key):
    return app.tasks.detail(human, ws, key)


def _rewrite(**overrides):
    rewrite = {
        "title": "Rewritten task",
        "description": "Do the thing.",
        "files": ["src/a.py", "src/b.py"],
        "acceptance_criteria": ["it works"],
        "size": "one-file",
    }
    rewrite.update(overrides)
    return rewrite


def test_resolved_comments_tags_and_cancels(app, human, ws):
    key = _task(app, human, ws, "Old bug")
    report = _apply(
        app,
        human,
        ws,
        [
            {
                "key": key,
                "verdict": "resolved",
                "summary": "Fixed in abc.",
                "evidence": ["git show abc", "a.py:1"],
            }
        ],
    )
    assert report["errors"] == []
    assert report["applied"] == [
        {"key": key, "verdict": "resolved", "action": "cancelled", "new_keys": [], "error": None}
    ]
    detail = _detail(app, human, ws, key)
    assert detail.status == "cancelled"
    assert "triage:resolved" in detail.tags
    comment = detail.comments[-1].body
    assert comment == (
        "Triage (triage batch TEST): resolved. Fixed in abc.\n\nEvidence:\n- git show abc\n- a.py:1"
    )


def test_obsolete_cancels_with_obsolete_tag(app, human, ws):
    key = _task(app, human, ws, "Gone")
    report = _apply(app, human, ws, [{"key": key, "verdict": "obsolete", "summary": "Module removed."}])
    assert report["applied"][0]["action"] == "cancelled"
    detail = _detail(app, human, ws, key)
    assert detail.status == "cancelled"
    assert "triage:obsolete" in detail.tags
    assert detail.comments[-1].body == "Triage (triage batch TEST): obsolete. Module removed."


def test_open_creates_follow_ups_and_supersedes_original(app, human, ws):
    key = _task(
        app,
        human,
        ws,
        "Partly done",
        tags=["wg136", "kind:bug", "area:core", "triaged-before"],
        priority="critical",
    )
    report = _apply(
        app,
        human,
        ws,
        [
            {
                "key": key,
                "verdict": "open",
                "summary": "Half is done.",
                "evidence": ["x.py:3"],
                "rewrites": [
                    _rewrite(),
                    _rewrite(title="Second part", files=[], acceptance_criteria=[], size="large"),
                ],
            }
        ],
    )
    assert report["errors"] == []
    new_keys = report["applied"][0]["new_keys"]
    assert len(new_keys) == 2
    assert report["applied"][0]["action"] == "superseded"

    first = _detail(app, human, ws, new_keys[0])
    assert first.title == "Rewritten task"
    assert first.priority == "critical"
    assert first.acceptance_criteria == ["it works"]
    # wg136 and kind:* are dropped from the original's tags; triaged and scope:* are added.
    assert sorted(first.tags) == ["area:core", "scope:one-file", "triaged", "triaged-before"]
    assert first.description == (
        f"Do the thing.\n\n**Files:** src/a.py, src/b.py\n\nTriaged from {key}: Half is done."
    )
    assert [r.relation_type for r in first.relations] == ["follows_up"]

    second = _detail(app, human, ws, new_keys[1])
    assert "**Files:**" not in second.description
    assert "scope:several-crates" in second.tags

    original = _detail(app, human, ws, key)
    assert original.status == "cancelled"
    assert "triage:superseded" in original.tags
    assert original.comments[-1].body.startswith(
        f"Triage (triage batch TEST): superseded by {', '.join(new_keys)}."
    )


@pytest.mark.parametrize(
    ("verdict", "extra", "tag", "lead"),
    [
        ("needs-decision", {"decision": "Pick A or B?"}, "needs-decision", "needs a decision"),
        ("needs-run", {"command": "cargo test -p rho"}, None, "needs a run to confirm"),
        ("unclear", {}, None, "unclear"),
    ],
)
def test_non_cancelling_verdicts_keep_status(app, human, ws, verdict, extra, tag, lead):
    key = _task(app, human, ws, "Ambiguous", status="ready")
    report = _apply(
        app,
        human,
        ws,
        [{"key": key, "verdict": verdict, "summary": "Undecided.", "evidence": ["e1"], **extra}],
    )
    assert report["errors"] == []
    assert report["applied"][0]["action"] == "commented"
    detail = _detail(app, human, ws, key)
    assert detail.status == "ready"
    assert f"triage:{verdict}" in detail.tags
    if tag:
        assert tag in detail.tags
    body = detail.comments[-1].body
    assert body.startswith(f"Triage (triage batch TEST): {lead}. Undecided.")
    assert body.endswith("Evidence:\n- e1")
    if verdict == "needs-decision":
        assert "Decision: Pick A or B?" in body
    if verdict == "needs-run":
        assert "Confirm with: cargo test -p rho" in body


@pytest.mark.parametrize(
    ("size", "tag"),
    [
        ("one-crate", "scope:one-crate"),
        ("several-crates", "scope:several-crates"),
        ("medium", "scope:one-crate"),
    ],
)
def test_rewrite_size_is_a_scope_and_legacy_sizes_map_to_one(app, human, ws, size, tag):
    key = _task(app, human, ws, "Scoped")
    report = _apply(
        app, human, ws, [{"key": key, "verdict": "open", "summary": "s", "rewrites": [_rewrite(size=size)]}]
    )
    assert report["errors"] == []
    assert tag in _detail(app, human, ws, report["applied"][0]["new_keys"][0]).tags


def test_unknown_rewrite_size_is_refused(app, human, ws):
    key = _task(app, human, ws, "Badly sized")
    report = _apply(
        app, human, ws, [{"key": key, "verdict": "open", "summary": "s", "rewrites": [_rewrite(size="huge")]}]
    )
    assert report["errors"][0]["error"] == "rewrite 0 has unknown size 'huge'"


def test_rerun_is_idempotent(app, human, ws):
    key = _task(app, human, ws, "Old")
    entries = [{"key": key, "verdict": "obsolete", "summary": "Gone."}]
    _apply(app, human, ws, entries)
    report = _apply(app, human, ws, entries)
    assert report == {
        "applied": [],
        "skipped": [{"key": key, "verdict": "obsolete", "reason": "already triaged (triage:obsolete)"}],
        "errors": [],
    }
    assert len(_detail(app, human, ws, key).comments) == 1


def test_dry_run_changes_nothing_and_reports_like_real_run(app, human, ws):
    cancel = _task(app, human, ws, "Cancel me")
    split = _task(app, human, ws, "Split me", tags=["wg136"])
    entries = [
        {"key": cancel, "verdict": "resolved", "summary": "Done."},
        {"key": split, "verdict": "open", "summary": "Split.", "rewrites": [_rewrite()]},
        {"key": "WG-9999", "verdict": "unclear", "summary": "?"},
    ]
    dry = _apply(app, human, ws, entries, dry_run=True)
    assert [a["action"] for a in dry["applied"]] == ["cancelled", "superseded"]
    assert dry["applied"][1]["new_keys"] == []
    assert len(dry["errors"]) == 1

    assert _detail(app, human, ws, cancel).status == "backlog"
    assert _detail(app, human, ws, cancel).comments == []
    assert _detail(app, human, ws, split).status == "backlog"

    real = _apply(app, human, ws, entries)
    assert real["errors"] == dry["errors"]
    assert [a["action"] for a in real["applied"]] == [a["action"] for a in dry["applied"]]
    assert _detail(app, human, ws, cancel).status == "cancelled"


def test_invalid_element_does_not_block_others(app, human, ws):
    good = _task(app, human, ws, "Good")
    accepted = _accepted(app, human, ws, "Already accepted")
    entries = [
        {"verdict": "resolved", "summary": "no key"},
        {"key": good, "verdict": "bogus", "summary": "?"},
        {"key": "WG-9999", "verdict": "unclear", "summary": "?"},
        {"key": good, "verdict": "open", "summary": "no rewrites", "rewrites": []},
        {"key": good, "verdict": "open", "summary": "bad rewrite", "rewrites": [{"description": "d"}]},
        {"key": good, "verdict": "needs-run", "summary": "no command"},
        {"key": accepted, "verdict": "obsolete", "summary": "too late"},
        {"key": good, "verdict": "resolved", "summary": "Fixed."},
    ]
    report = _apply(app, human, ws, entries)
    assert [e["index"] for e in report["errors"]] == [0, 1, 2, 3, 4, 5, 6]
    assert "missing key" in report["errors"][0]["error"]
    assert "unknown verdict" in report["errors"][1]["error"]
    assert "not found" in report["errors"][2]["error"]
    assert "rewrite" in report["errors"][4]["error"]
    assert "command" in report["errors"][5]["error"]
    assert "cannot move" in report["errors"][6]["error"]
    assert [a["key"] for a in report["applied"]] == [good]
    assert _detail(app, human, ws, good).status == "cancelled"
    assert _detail(app, human, ws, accepted).status == "accepted"


def test_duplicate_key_in_file_is_an_error(app, human, ws):
    key = _task(app, human, ws, "Twice")
    report = _apply(
        app,
        human,
        ws,
        [
            {"key": key, "verdict": "unclear", "summary": "a"},
            {"key": key.lower(), "verdict": "unclear", "summary": "b"},
        ],
    )
    assert len(report["applied"]) == 1
    assert report["errors"][0]["index"] == 1
    assert "duplicate" in report["errors"][0]["error"]


def test_rewrite_tag_rules_follow_only_listed_tags(app, human, ws):
    key = _task(app, human, ws, "Tags", tags=["kind:feature", "wg136", "keep"])
    report = _apply(
        app,
        human,
        ws,
        [{"key": key, "verdict": "open", "summary": "s", "rewrites": [_rewrite(size=None)]}],
    )
    child = _detail(app, human, ws, report["applied"][0]["new_keys"][0])
    assert sorted(child.tags) == ["keep", "triaged"]


def test_load_verdicts_requires_array(tmp_path: Path):
    path = tmp_path / "v.json"
    path.write_text(json.dumps({"key": "WG-1"}), encoding="utf-8")
    from workgraph.core.errors import ValidationFailed

    with pytest.raises(ValidationFailed):
        triage.load_verdicts(path)


@pytest.fixture
def cli_run(home, monkeypatch):
    monkeypatch.setenv("WORKGRAPH_HOME", str(home))
    runner = CliRunner()

    def invoke(*args: str, expect: int = 0, json_out: bool = True) -> object:
        STATE.json = json_out
        STATE.home = None
        try:
            result = runner.invoke(cli, list(args), catch_exceptions=False)
        finally:
            STATE.close()
            STATE.json = False
        assert result.exit_code == expect, f"{args}: exit {result.exit_code}\n{result.output}"
        return result.output

    return invoke


def test_cli_triage_apply_json_and_exit_codes(cli_run, tmp_path: Path):
    cli_run("init", "--workspace", "acme", "--prefix", "AC")
    first = json.loads(cli_run("task", "create", "Old bug"))
    second = json.loads(cli_run("task", "create", "Other"))
    path = tmp_path / "verdicts.json"
    path.write_text(
        json.dumps(
            [
                {"key": first["key"], "verdict": "resolved", "summary": "Fixed."},
                {"key": second["key"], "verdict": "maybe", "summary": "?"},
            ]
        ),
        encoding="utf-8",
    )
    out = json.loads(cli_run("task", "triage-apply", str(path), "--source", "triage batch T", expect=1))
    assert [a["key"] for a in out["applied"]] == [first["key"]]
    assert out["applied"][0]["action"] == "cancelled"
    assert out["skipped"] == []
    assert len(out["errors"]) == 1
    assert out["errors"][0]["index"] == 1

    # Re-running with only the good element is a skip, which exits 0.
    path.write_text(
        json.dumps([{"key": first["key"], "verdict": "resolved", "summary": "Fixed."}]), encoding="utf-8"
    )
    again = json.loads(cli_run("task", "triage-apply", str(path)))
    assert again["applied"] == []
    assert again["skipped"][0]["key"] == first["key"]
    assert again["errors"] == []


def test_cli_triage_apply_table_output(cli_run, tmp_path: Path):
    cli_run("init", "--workspace", "acme", "--prefix", "AC")
    made = json.loads(cli_run("task", "create", "Maybe"))
    path = tmp_path / "v.json"
    path.write_text(
        json.dumps([{"key": made["key"], "verdict": "needs-run", "summary": "s", "command": "pytest -q"}]),
        encoding="utf-8",
    )
    text = cli_run("task", "triage-apply", str(path), "--dry-run", json_out=False)
    assert "needs-run" in text
    assert "nothing changed" in text
