from __future__ import annotations

import pytest

from workgraph.core.errors import ValidationFailed
from workgraph.core.services.breakdown import parse_items, section_lines

# Shaped like section 9 of docs/rust-rewrite.md: bold headers, a ranged block with a table, and
# "Deps:" clauses that range over ids.
RANGED_DOC = """\
# Plan

## 9. Task breakdown

Intro text that is not a task.

**FD-01 Cargo workspace skeleton**
Touches: root `Cargo.toml`. Accept: `cargo check` passes. Deps: none.

**GV-01 to GV-03 Golden vectors** (each: a deterministic script that writes `golden/<name>/`).
Deps: FD-01.

| Id | Name | Python source |
| --- | --- | --- |
| GV-01 | `lifecycle` | `core/policies/lifecycle.py` |
| GV-02 | `signatures` | `verification/service.py` |
| GV-03 | `gitparse` | `integrations/git/parse.py` |

**SC-01 to SC-03 Scenarios** (each: scenario files). Deps: GV-01 to GV-03 (GV-03 for the database comparisons).

| Id | Source or purpose |
| --- | --- |
| SC-01 | `test_git.py` |
| SC-02 | `test_worktrees.py` |
| SC-03 | `test_runs.py` |

**CORE-14 Stack integration** Deps: SC-01 to SC-03, FD-01. Tier: top.

## 10. Appendix

**ZZ-01 Not part of the breakdown** Deps: CORE-14.
"""

# Shaped like a document with "**V1. Title**" blocks and "* Depends on: ..." bullets.
VBLOCK_DOC = """\
## Work

**V1. Parse the input**

* Depends on: V4. Tier: mid.

Read the file.

**V4. Schema**
* Depends on: none.
* Tier: top

Define the schema, then the parser can start.

**V5. Report**

* Depends on: V1, V4. Q1 answered.

Write the report.

```
**V9. Inside a code fence, not a task**
```
"""


def test_section_ends_at_next_heading_of_same_or_higher_level():
    lines = section_lines(RANGED_DOC, "9. Task breakdown")
    text = "\n".join(lines)
    assert "**FD-01 Cargo workspace skeleton**" in text
    assert "**CORE-14 Stack integration**" in text
    assert "ZZ-01" not in text
    assert "## 10" not in text


def test_section_includes_deeper_subheadings():
    doc = "## A\n\n### A.1\n\n**V1. One**\n\n## B\n\n**V2. Two**\n"
    text = "\n".join(section_lines(doc, "A"))
    assert "### A.1" in text
    assert "**V2. Two**" not in text


def test_section_heading_must_exist_and_be_unique():
    with pytest.raises(ValidationFailed, match="no heading"):
        section_lines(RANGED_DOC, "Missing")
    with pytest.raises(ValidationFailed, match="occurs 2 times"):
        section_lines("## X\n\n## X\n", "X")


def test_ranged_block_yields_one_task_per_row_with_block_text():
    items = parse_items(section_lines(RANGED_DOC, "9. Task breakdown"))
    by_id = {item.id: item for item in items}
    assert [item.id for item in items] == [
        "FD-01",
        "GV-01",
        "GV-02",
        "GV-03",
        "SC-01",
        "SC-02",
        "SC-03",
        "CORE-14",
    ]
    assert by_id["GV-02"].title == "Golden vectors: `signatures`"
    assert by_id["GV-02"].block == "GV-01"
    assert by_id["GV-02"].description.startswith("(each: a deterministic script")
    assert by_id["GV-02"].description.endswith("| GV-02 | `signatures` | `verification/service.py` |")
    # The table itself is not repeated in every row's description.
    assert "| GV-01 |" not in by_id["GV-02"].description


def test_deps_ranges_expand_and_tier_becomes_tag_candidate():
    by_id = {item.id: item for item in parse_items(section_lines(RANGED_DOC, "9. Task breakdown"))}
    assert by_id["FD-01"].deps == []
    assert by_id["GV-01"].deps == ["FD-01"]
    # A parenthetical is kept apart: its ids are reported as text, not silently dropped.
    assert by_id["SC-02"].deps == ["GV-01", "GV-02", "GV-03", "GV-03 for the database comparisons"]
    assert by_id["CORE-14"].deps == ["SC-01", "SC-02", "SC-03", "FD-01"]
    assert by_id["CORE-14"].tier_tags == ["tier-top"]
    assert by_id["FD-01"].tier_tags == []


def test_inline_depends_and_tier_bullets():
    items = {item.id: item for item in parse_items(section_lines(VBLOCK_DOC, "Work"))}
    assert set(items) == {"V1", "V4", "V5"}
    assert items["V1"].deps == ["V4"]
    assert items["V1"].tier_tags == ["tier-mid"]
    assert items["V4"].deps == []
    assert items["V4"].tier_tags == ["tier-top"]
    assert items["V5"].deps == ["V1", "V4", "Q1 answered"]
    assert items["V5"].tier_tags == []


def test_fenced_header_lookalikes_are_not_tasks():
    items = parse_items(section_lines(VBLOCK_DOC, "Work"))
    assert "V9" not in [item.id for item in items]


def test_parenthetical_dependency_is_reported_as_text():
    doc = "## S\n\n**A1 Thing** Deps: B1 (for the database comparisons).\n\n**B1 Other**\n"
    items = {item.id: item for item in parse_items(section_lines(doc, "S"))}
    assert items["A1"].deps == ["B1", "for the database comparisons"]


def test_ranged_block_without_table_is_refused():
    doc = "## S\n\n**GV-01 to GV-03 Nothing here**\n\nJust prose.\n"
    with pytest.raises(ValidationFailed, match="has no table rows"):
        parse_items(section_lines(doc, "S"))


def test_duplicate_ids_are_refused():
    doc = "## S\n\n**V1. One**\n\n**V1 Again**\n"
    with pytest.raises(ValidationFailed, match="appears more than once"):
        parse_items(section_lines(doc, "S"))


def test_deps_and_tier_columns_apply_per_row():
    # Shaped like the interface tables of docs/rust-rewrite.md: a Deps column and a Tier column.
    doc = """\
## S

**CORE-02 to CORE-03 Domain crates** (each: pure functions)

| Id | Crate: scope | Deps | Tier |
| --- | --- | --- | --- |
| CORE-02 | `wg-tasks`: graph | CORE-01, LIB-10 | mid |
| CORE-03 | `wg-tasks`: create | CORE-02 | haiku |
"""
    items = {item.id: item for item in parse_items(section_lines(doc, "S"))}
    assert items["CORE-02"].deps == ["CORE-01", "LIB-10"]
    assert items["CORE-02"].tier_tags == ["tier-mid"]
    assert items["CORE-03"].deps == ["CORE-02"]
    assert items["CORE-03"].tier_tags == []
    assert items["CORE-03"].title == "Domain crates: `wg-tasks`: create"
