//! `task import-breakdown`: the parser and the plan. Ports `tests/unit/test_breakdown.py` and the
//! planning cases of `tests/integration/test_breakdown.py` as decisions on values.

use std::collections::HashMap;
use wg_tasks::breakdown::{
    DESCRIPTION_DIGEST, Existing, Item, PlanInput, Ref, description_digest, external_ids,
    parse_items, plan, section_lines,
};
use wg_types::ErrorCode;

const RANGED_DOC: &str = "\
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
";

const VBLOCK_DOC: &str = "\
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
";

const SUBSECTION_DOC: &str = "\
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
";

const MULTI_COLUMN_DOC: &str = "\
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
";

fn items_of(doc: &str, section: &str) -> Vec<Item> {
    let lines = section_lines(doc, section).unwrap_or_else(|e| panic!("section: {e}"));
    parse_items(&lines).unwrap_or_else(|e| panic!("items: {e}"))
}

fn by_id(items: &[Item]) -> HashMap<&str, &Item> {
    items.iter().map(|i| (i.id.as_str(), i)).collect()
}

fn strs(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

fn ids(items: &[Item]) -> Vec<&str> {
    items.iter().map(|i| i.id.as_str()).collect()
}

#[test]
fn section_ends_at_next_heading_of_same_or_higher_level() {
    let text = section_lines(RANGED_DOC, "9. Task breakdown")
        .unwrap_or_default()
        .join("\n");
    assert!(text.contains("**FD-01 Cargo workspace skeleton**"));
    assert!(text.contains("**CORE-14 Stack integration**"));
    assert!(!text.contains("ZZ-01"));
    assert!(!text.contains("## 10"));
}

#[test]
fn section_includes_deeper_subheadings() {
    let doc = "## A\n\n### A.1\n\n**V1. One**\n\n## B\n\n**V2. Two**\n";
    let text = section_lines(doc, "A").unwrap_or_default().join("\n");
    assert!(text.contains("### A.1"));
    assert!(!text.contains("**V2. Two**"));
}

#[test]
fn section_heading_must_exist_and_be_unique() {
    let missing = section_lines(RANGED_DOC, "Missing").err();
    assert_eq!(
        missing.as_ref().map(|e| e.code),
        Some(ErrorCode::ValidationFailed)
    );
    assert!(missing.is_some_and(|e| e.message.contains("no heading 'Missing'")));
    let twice = section_lines("## X\n\n## X\n", "X").err();
    assert!(twice.is_some_and(|e| e.message.contains("occurs 2 times")));
}

#[test]
fn ranged_block_yields_one_task_per_row_with_block_text() {
    let items = items_of(RANGED_DOC, "9. Task breakdown");
    assert_eq!(
        ids(&items),
        [
            "FD-01", "GV-01", "GV-02", "GV-03", "SC-01", "SC-02", "SC-03", "CORE-14"
        ]
    );
    let gv2 = by_id(&items)["GV-02"];
    assert_eq!(gv2.title, "Golden vectors: `signatures`");
    assert!(gv2.description.starts_with("(each: a deterministic script"));
    assert!(
        gv2.description
            .ends_with("| GV-02 | `signatures` | `verification/service.py` |")
    );
    assert!(!gv2.description.contains("| GV-01 |"));
}

#[test]
fn deps_ranges_expand_and_tier_becomes_tag_candidate() {
    let items = items_of(RANGED_DOC, "9. Task breakdown");
    let m = by_id(&items);
    assert!(m["FD-01"].deps.is_empty());
    assert_eq!(m["GV-01"].deps, strs(&["FD-01"]));
    assert_eq!(
        m["SC-02"].deps,
        strs(&[
            "GV-01",
            "GV-02",
            "GV-03",
            "GV-03 for the database comparisons"
        ])
    );
    assert_eq!(
        m["CORE-14"].deps,
        strs(&["SC-01", "SC-02", "SC-03", "FD-01"])
    );
    assert_eq!(m["CORE-14"].tier_tags, strs(&["tier-top"]));
    assert!(m["FD-01"].tier_tags.is_empty());
}

#[test]
fn inline_depends_and_tier_bullets() {
    let items = items_of(VBLOCK_DOC, "Work");
    let m = by_id(&items);
    assert_eq!(ids(&items), ["V1", "V4", "V5"]);
    assert_eq!(m["V1"].deps, strs(&["V4"]));
    assert_eq!(m["V1"].tier_tags, strs(&["tier-mid"]));
    assert!(m["V4"].deps.is_empty());
    assert_eq!(m["V4"].tier_tags, strs(&["tier-top"]));
    assert_eq!(m["V5"].deps, strs(&["V1", "V4", "Q1 answered"]));
    assert!(m["V5"].tier_tags.is_empty());
}

#[test]
fn parenthetical_dependency_is_reported_as_text() {
    let doc = "## S\n\n**A1 Thing** Deps: B1 (for the database comparisons).\n\n**B1 Other**\n";
    let items = items_of(doc, "S");
    assert_eq!(
        by_id(&items)["A1"].deps,
        strs(&["B1", "for the database comparisons"])
    );
}

#[test]
fn ranged_block_without_table_is_refused() {
    let doc = "## S\n\n**GV-01 to GV-03 Nothing here**\n\nJust prose.\n";
    let lines = section_lines(doc, "S").unwrap_or_default();
    let err = parse_items(&lines).err();
    assert!(err.is_some_and(|e| e.message.contains("has no table rows")));
}

#[test]
fn duplicate_ids_are_refused() {
    let doc = "## S\n\n**V1. One**\n\n**V1 Again**\n";
    let lines = section_lines(doc, "S").unwrap_or_default();
    let err = parse_items(&lines).err();
    assert!(err.is_some_and(|e| e.message.contains("appears more than once")));
}

#[test]
fn row_description_has_only_its_own_subsection_prose() {
    let items = items_of(SUBSECTION_DOC, "9. Breakdown");
    assert_eq!(ids(&items), ["AA-01", "AA-02", "BB-01", "BB-02", "CC-01"]);
    let m = by_id(&items);
    let aa = &m["AA-02"].description;
    assert!(aa.contains("Prose for first.") && aa.contains("(each: alpha work)"));
    for absent in ["second", "Beta", "beta work", "9.2", "third"] {
        assert!(!aa.contains(absent), "{absent} in {aa}");
    }
    let bb = &m["BB-01"].description;
    assert!(bb.contains("Prose for second.") && bb.contains("(each: beta work)"));
    for absent in ["first", "alpha", "third"] {
        assert!(!bb.contains(absent), "{absent} in {bb}");
    }
    assert_eq!(
        m["CC-01"].description,
        "Prose for third.\n\n| CC-01 | `init` |"
    );
}

#[test]
fn summary_table_under_subheading_is_not_a_task_table() {
    let doc = "## 9\n\n### 9.2 Levels\n\n| Level | Tasks | Count |\n| --- | --- | --- |\n| L0 | FD-01, CF-01 | 2 |\n";
    assert!(items_of(doc, "9").is_empty());
}

#[test]
fn table_under_subheading_without_bold_header_is_a_task_table() {
    let items = items_of(SUBSECTION_DOC, "9. Breakdown");
    assert_eq!(by_id(&items)["CC-01"].title, "9.3 Third: `init`");
}

#[test]
fn deps_and_tier_columns_apply_per_row() {
    let doc = "\
## S

**CORE-02 to CORE-03 Domain crates** (each: pure functions)

| Id | Crate: scope | Deps | Tier |
| --- | --- | --- | --- |
| CORE-02 | `wg-tasks`: graph | CORE-01, LIB-10 | mid |
| CORE-03 | `wg-tasks`: create | CORE-02 | haiku |
";
    let items = items_of(doc, "S");
    let m = by_id(&items);
    assert_eq!(m["CORE-02"].deps, strs(&["CORE-01", "LIB-10"]));
    assert_eq!(m["CORE-02"].tier_tags, strs(&["tier-mid"]));
    assert_eq!(m["CORE-03"].deps, strs(&["CORE-02"]));
    assert!(m["CORE-03"].tier_tags.is_empty());
    assert_eq!(m["CORE-03"].title, "Domain crates: `wg-tasks`: create");
}

#[test]
fn table_dependency_column_is_read_by_header_name() {
    let items = items_of(MULTI_COLUMN_DOC, "S");
    let m = by_id(&items);
    assert!(m["IF-01"].deps.is_empty());
    assert_eq!(m["IF-02"].deps, strs(&["IF-01", "LIB-09"]));
    assert_eq!(m["CF-01"].deps, strs(&["IF-01"]));
    assert!(m["CF-02"].deps.is_empty());
}

#[test]
fn tier_column_is_not_a_dependency() {
    let items = items_of(MULTI_COLUMN_DOC, "S");
    let m = by_id(&items);
    assert_eq!(m["IF-02"].tier_tags, strs(&["tier-top"]));
    assert_eq!(m["IF-01"].tier_tags, strs(&["tier-mid"]));
    assert_eq!(m["CF-02"].tier_tags, strs(&["tier-top"]));
}

#[test]
fn id_range_in_a_dependency_cell_expands() {
    let items = items_of(MULTI_COLUMN_DOC, "S");
    assert_eq!(by_id(&items)["IF-03"].deps, strs(&["IF-01", "IF-02"]));
}

#[test]
fn all_prefixes_phrase_expands_to_section_ids_of_those_prefixes() {
    let items = items_of(MULTI_COLUMN_DOC, "S");
    assert_eq!(
        by_id(&items)["SC-01"].deps,
        strs(&["IF-01", "IF-02", "IF-03"])
    );
}

#[test]
fn all_phrase_matching_no_id_stays_as_text() {
    let items = items_of(MULTI_COLUMN_DOC, "S");
    assert_eq!(by_id(&items)["XX-01"].deps, strs(&["all QQ tasks"]));
}

// The planning cases of the integration tests.

fn task(id: &str, key: &str, status: &str, item: &Item) -> Existing {
    Existing {
        id: id.to_owned(),
        key: key.to_owned(),
        title: item.title.clone(),
        description: item.description.clone(),
        status: status.to_owned(),
        tags: Vec::new(),
        parent_id: Some("P".to_owned()),
        depends_on: Vec::new(),
        digest: Some(description_digest(&item.description)),
    }
}

const PLAN_DOC: &str = "\
## S

**A1 Waits** Deps: B1.

**B1 First**

**C1 Needs the seed** Deps: SEED-1.
";

fn plan_for(
    items: &[Item],
    existing: &HashMap<String, Existing>,
    lookups: &HashMap<String, Option<Existing>>,
) -> wg_tasks::breakdown::Plan {
    plan(&PlanInput {
        items,
        existing,
        lookups,
        parent: Some(("EP-1", "P")),
        tags: &strs(&["imported"]),
    })
}

fn seed(status: &str) -> Existing {
    Existing {
        id: "seed-id".into(),
        key: "SEED-1".into(),
        title: "Seed".into(),
        description: String::new(),
        status: status.into(),
        tags: Vec::new(),
        parent_id: None,
        depends_on: Vec::new(),
        digest: None,
    }
}

#[test]
fn new_items_are_created_ready_only_when_every_dependency_is_accepted() {
    let items = items_of(PLAN_DOC, "S");
    assert_eq!(external_ids(&items), strs(&["SEED-1"]));
    let lookups = HashMap::from([("SEED-1".to_owned(), Some(seed("accepted")))]);
    let p = plan_for(&items, &HashMap::new(), &lookups);
    let status: Vec<(&str, &str)> = p
        .entries
        .iter()
        .map(|e| (e.item.id.as_str(), e.status.as_str()))
        .collect();
    assert_eq!(
        status,
        [("A1", "backlog"), ("B1", "ready"), ("C1", "ready")]
    );
    assert!(p.entries.iter().all(|e| e.action == "created"));
    assert!(p.unresolved.is_empty());
    assert_eq!(p.entries[0].desired_tags, strs(&["imported"]));
}

#[test]
fn unknown_key_is_unresolved_and_keeps_the_task_in_backlog() {
    let items = items_of(PLAN_DOC, "S");
    let lookups = HashMap::from([("SEED-1".to_owned(), None)]);
    let p = plan_for(&items, &HashMap::new(), &lookups);
    assert_eq!(p.unresolved.len(), 1);
    assert_eq!(p.unresolved[0].dependency, "SEED-1");
    assert_eq!(
        p.unresolved[0].reason,
        "no such task in this section or the workspace"
    );
    assert_eq!(p.entries[2].status, "backlog");
}

#[test]
fn free_text_dependency_is_not_a_task_id() {
    let doc = "## S\n\n**V2. Report**\n\n* Depends on: Q1 answered.\n";
    let items = items_of(doc, "S");
    let p = plan_for(&items, &HashMap::new(), &HashMap::new());
    assert_eq!(p.unresolved[0].reason, "not a task id");
    assert_eq!(p.entries[0].status, "backlog");
}

fn imported(items: &[Item], dep_ids: &[(&str, &str)]) -> HashMap<String, Existing> {
    items
        .iter()
        .enumerate()
        .map(|(n, item)| {
            let status = if item.deps.is_empty() {
                "ready"
            } else {
                "backlog"
            };
            let mut t = task(&format!("id{n}"), &format!("K-{n}"), status, item);
            t.tags = strs(&["imported"]);
            t.depends_on = dep_ids
                .iter()
                .filter(|(who, _)| *who == item.id)
                .map(|(_, dep)| (*dep).to_owned())
                .collect();
            (item.id.clone(), t)
        })
        .collect()
}

#[test]
fn rerun_with_nothing_changed_is_unchanged() {
    let items = items_of(PLAN_DOC, "S");
    let lookups = HashMap::from([("SEED-1".to_owned(), Some(seed("backlog")))]);
    let existing = imported(&items, &[("A1", "id1"), ("C1", "seed-id")]);
    let p = plan_for(&items, &existing, &lookups);
    assert!(
        p.entries.iter().all(|e| e.action == "unchanged"),
        "{:?}",
        p.entries.iter().map(|e| e.action).collect::<Vec<_>>()
    );
    assert!(p.kept.is_empty());
}

#[test]
fn rerun_adds_a_missing_dependency_as_an_update() {
    let items = items_of(PLAN_DOC, "S");
    let lookups = HashMap::from([("SEED-1".to_owned(), Some(seed("backlog")))]);
    let existing = imported(&items, &[("C1", "seed-id")]);
    let p = plan_for(&items, &existing, &lookups);
    assert_eq!(p.entries[0].action, "updated");
    assert_eq!(p.entries[0].deps_to_add, [Ref::Section("B1".into())]);
    assert_eq!(p.entries[1].action, "unchanged");
}

#[test]
fn rerun_promotes_backlog_task_once_dependency_is_accepted() {
    let items = items_of("## S\n\n**A1 Waits** Deps: B1.\n\n**B1 First**\n", "S");
    let mut existing = imported(&items, &[("A1", "id1")]);
    if let Some(b1) = existing.get_mut("B1") {
        b1.status = "accepted".into();
    }
    let p = plan_for(&items, &existing, &HashMap::new());
    assert_eq!(p.entries[0].status, "ready");
    assert_eq!(p.entries[0].action, "updated");
}

#[test]
fn rerun_replaces_an_unedited_description_and_keeps_an_edited_one() {
    let doc = "## S\n\n**A1 One**\n\nOld text.\n\n**B1 Two**\n\nOld two.\n";
    let old = items_of(doc, "S");
    let mut existing = imported(&old, &[]);
    if let Some(b1) = existing.get_mut("B1") {
        b1.description = "Hand-written note.".into();
    }
    let new = items_of(
        &doc.replace("Old text.", "New text.")
            .replace("Old two.", "New two."),
        "S",
    );
    let p = plan_for(&new, &existing, &HashMap::new());
    assert_eq!(p.entries[0].action, "updated");
    assert_eq!(
        p.entries[0].changes.description.as_deref(),
        Some("New text.")
    );
    assert!(p.entries[0].changes.digest.is_some());
    assert_eq!(p.entries[1].action, "unchanged");
    assert_eq!(p.kept.len(), 1);
    assert_eq!(p.kept[0].key, "K-1");
    assert_eq!(p.kept[0].reason, "description edited since import");
    assert_eq!(DESCRIPTION_DIGEST, "breakdown_description_sha256");
}

#[test]
fn tier_tags_that_left_the_document_are_removed() {
    let items = items_of("## S\n\n**A1 One** Tier: mid\n", "S");
    let mut existing = imported(&items, &[]);
    if let Some(a1) = existing.get_mut("A1") {
        a1.tags = strs(&["tier-top", "imported"]);
    }
    let p = plan_for(&items, &existing, &HashMap::new());
    assert_eq!(p.entries[0].add_tags, strs(&["tier-mid"]));
    assert_eq!(p.entries[0].remove_tags, strs(&["tier-top"]));
}
