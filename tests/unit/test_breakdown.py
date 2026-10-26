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


# Shaped like section 9 of docs/rust-rewrite.md: each "### 9.x" subsection has its own prose, bold
# ranged blocks and tables, and a table may sit under its subheading with no bold header above it.
SUBSECTION_DOC = """\
## 9. Breakdown

### 9.1 First

Prose for first.

**AA-01 to AA-02 Alpha** (each: alpha work).

| Id | Name |
| --- | --- |
| AA-01 | one |
| AA-02 | two |

### 9.2 Second

Prose for second.

**BB-01 to BB-02 Beta** (each: beta work).

| Id | Name |
| --- | --- |
| BB-01 | three |
| BB-02 | four |

### 9.3 Third

Prose for third.

| Id | Commands |
| --- | --- |
| CC-01 | `init` |
"""


def test_row_description_has_only_its_own_subsection_prose():
    items = {item.id: item for item in parse_items(section_lines(SUBSECTION_DOC, "9. Breakdown"))}
    assert set(items) == {"AA-01", "AA-02", "BB-01", "BB-02", "CC-01"}
    aa = items["AA-02"].description
    assert "Prose for first." in aa and "(each: alpha work)" in aa
    assert "second" not in aa and "Beta" not in aa and "beta work" not in aa
    assert "9.2" not in aa and "third" not in aa
    bb = items["BB-01"].description
    assert "Prose for second." in bb and "(each: beta work)" in bb
    assert "first" not in bb and "alpha" not in bb and "third" not in bb
    cc = items["CC-01"].description
    assert cc == "Prose for third.\n\n| CC-01 | `init` |"


def test_summary_table_under_subheading_is_not_a_task_table():
    doc = "## 9\n\n### 9.2 Levels\n\n| Level | Tasks | Count |\n| --- | --- | --- |\n| L0 | FD-01, CF-01 | 2 |\n"
    assert parse_items(section_lines(doc, "9")) == []


def test_table_under_subheading_without_bold_header_is_a_task_table():
    items = {item.id: item for item in parse_items(section_lines(SUBSECTION_DOC, "9. Breakdown"))}
    assert items["CC-01"].title == "9.3 Third: `init`"
    assert items["CC-01"].block is None
    assert items["AA-01"].block == "AA-01"


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


# A multi-column table: Contracts is not a dependency column, Deps is, Tier follows Deps.
MULTI_COLUMN_DOC = """\
## S

**IF-01 to IF-03 Interface** (each: a module)

| Id | Commands | Contracts | Deps | Tier |
| --- | --- | --- | --- | --- |
| IF-01 | `init` | C001-C003 | none | mid |
| IF-02 | `init`, `doctor` | C058-C062, C065 | IF-01, LIB-09 | top |
| IF-03 | `status` | C070 | IF-01 to IF-02 | mid |

**SC-01 Scenarios** Deps: all IF and SC tasks.

**CF-01 to CF-02 Config**

| Id | Contracts | Dependencies | Tier |
| --- | --- | --- | --- |
| CF-01 | C090 | IF-01 | mid |
| CF-02 | C091 | none | top |

**XX-01 Unknown group** Deps: all QQ tasks.
"""


def test_table_dependency_column_is_read_by_header_name():
    items = {item.id: item for item in parse_items(section_lines(MULTI_COLUMN_DOC, "S"))}
    # Contracts ("C058-C062, C065") is ignored; the Deps cell is read, and "none" means no dependency.
    assert items["IF-01"].deps == []
    assert items["IF-02"].deps == ["IF-01", "LIB-09"]
    assert "C058-C062" not in items["IF-02"].deps
    assert items["CF-01"].deps == ["IF-01"]
    assert items["CF-02"].deps == []


def test_tier_column_is_not_a_dependency():
    items = {item.id: item for item in parse_items(section_lines(MULTI_COLUMN_DOC, "S"))}
    assert items["IF-02"].tier_tags == ["tier-top"]
    assert items["IF-02"].deps == ["IF-01", "LIB-09"]
    assert items["IF-01"].tier_tags == ["tier-mid"]
    assert "mid" not in items["IF-01"].deps and "top" not in items["IF-02"].deps
    assert items["CF-02"].tier_tags == ["tier-top"]


def test_id_range_in_a_dependency_cell_expands():
    items = {item.id: item for item in parse_items(section_lines(MULTI_COLUMN_DOC, "S"))}
    assert items["IF-03"].deps == ["IF-01", "IF-02"]


def test_all_prefixes_phrase_expands_to_section_ids_of_those_prefixes():
    items = {item.id: item for item in parse_items(section_lines(MULTI_COLUMN_DOC, "S"))}
    # Every IF task of the section; the SC task does not depend on itself.
    assert items["SC-01"].deps == ["IF-01", "IF-02", "IF-03"]


def test_all_phrase_matching_no_id_stays_as_text():
    items = {item.id: item for item in parse_items(section_lines(MULTI_COLUMN_DOC, "S"))}
    assert items["XX-01"].deps == ["all QQ tasks"]
