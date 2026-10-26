//! CR-07: which contract ids of the inventory the conformance corpus covers.
//!
//! The inventory is `docs/rust-rewrite-contracts.md`: every table row whose first cell is an id
//! (`C001`, `M01`, `X1`, ...). Every scenario declares the ids it exercises in `covers`. This
//! module joins the two and reports, per id, the scenarios that cover it and the ids that nothing
//! covers. It is pure: it takes text and values, and the file access lives in `main.rs`.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// The ids of the inventory, sorted and unique. An id is the first cell of a markdown table row,
/// optionally wrapped in `**`, made of upper-case letters followed by digits.
pub fn inventory_ids(markdown: &str) -> Vec<String> {
    let ids: BTreeSet<String> = markdown
        .lines()
        .filter(|line| line.starts_with('|'))
        .filter_map(|line| line.split('|').nth(1))
        .map(|cell| cell.trim().trim_matches('*').trim())
        .filter(|cell| is_id(cell))
        .map(str::to_owned)
        .collect();
    ids.into_iter().collect()
}

fn is_id(s: &str) -> bool {
    let letters = s.chars().take_while(char::is_ascii_uppercase).count();
    letters > 0 && s.len() > letters && s[letters..].chars().all(|c| c.is_ascii_digit())
}

/// The inventory joined with the scenarios that cover it.
#[derive(Debug, Default, PartialEq)]
pub struct Coverage {
    /// Every inventory id, with the scenarios whose `covers` name it (empty when uncovered).
    pub by_id: BTreeMap<String, Vec<String>>,
    /// Ids that scenarios name but the inventory does not list, with the scenarios naming them.
    pub unknown: BTreeMap<String, Vec<String>>,
}

impl Coverage {
    /// Inventory ids that no scenario covers, in id order.
    pub fn uncovered(&self) -> Vec<&str> {
        self.by_id
            .iter()
            .filter(|(_, scenarios)| scenarios.is_empty())
            .map(|(id, _)| id.as_str())
            .collect()
    }
}

/// Join the inventory with `(scenario id, covers)` pairs. A scenario counts once per id it names,
/// and lists keep the order of `scenarios`.
pub fn coverage(inventory: &[String], scenarios: &[(String, Vec<String>)]) -> Coverage {
    let mut by_id: BTreeMap<String, Vec<String>> = inventory
        .iter()
        .map(|id| (id.clone(), Vec::new()))
        .collect();
    let mut unknown: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (scenario, covers) in scenarios {
        let distinct: BTreeSet<&String> = covers.iter().collect();
        for id in distinct {
            match by_id.get_mut(id) {
                Some(list) => list.push(scenario.clone()),
                None => unknown
                    .entry(id.clone())
                    .or_default()
                    .push(scenario.clone()),
            }
        }
    }
    Coverage { by_id, unknown }
}

/// Human report: the covered ids with their scenarios, then the uncovered ids, then any ids the
/// inventory does not list, then the scenario files that failed to load.
pub fn report_text(c: &Coverage, invalid: &[String]) -> String {
    let total = c.by_id.len();
    let uncovered = c.uncovered();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "coverage: {} of {total} contract ids covered, {} uncovered",
        total - uncovered.len(),
        uncovered.len()
    );
    for (id, scenarios) in c.by_id.iter().filter(|(_, s)| !s.is_empty()) {
        let _ = writeln!(out, "{id}  {}", scenarios.join(", "));
    }
    if !uncovered.is_empty() {
        out.push_str("uncovered:\n");
        for id in &uncovered {
            let _ = writeln!(out, "  {id}");
        }
    }
    if !c.unknown.is_empty() {
        out.push_str("not in the inventory:\n");
        for (id, scenarios) in &c.unknown {
            let _ = writeln!(out, "  {id}  {}", scenarios.join(", "));
        }
    }
    if !invalid.is_empty() {
        out.push_str("invalid scenarios (not counted):\n");
        for problem in invalid {
            let _ = writeln!(out, "  {problem}");
        }
    }
    out
}

/// Machine report: the same content as [`report_text`], as JSON.
pub fn report_json(c: &Coverage, invalid: &[String]) -> Value {
    let uncovered = c.uncovered();
    json!({
        "total": c.by_id.len(),
        "covered": c.by_id.len() - uncovered.len(),
        "uncovered": uncovered,
        "ids": c.by_id,
        "unknown": c.unknown,
        "invalid": invalid,
    })
}
