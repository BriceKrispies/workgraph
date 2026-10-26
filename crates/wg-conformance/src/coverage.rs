//! CR-07: which contract ids of the inventory the conformance corpus and the test suites cover.
//!
//! The inventory is `docs/rust-rewrite-contracts.md`: every table row whose first cell is an id
//! (`C001`, `M01`, `X1`, ...). Three kinds of source name the ids they cover:
//!
//! * scenarios, through their `covers` list;
//! * Rust and Python tests, through a marker comment `covers: C001, M02` placed directly above the
//!   test function (attribute and decorator lines may sit between the two);
//! * an allowlist of ids that are not observable, each with the reason (`coverage-allowlist.toml`).
//!
//! This module is pure: it takes text and values and reports what it found. The file access lives
//! in `main.rs`.

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

/// The `covers` markers of one file: each test function that a marker sits above, as its source
/// name (`file::fn`) with the ids it names. Problems are markers that name something other than a
/// contract id, or that are not followed by a test function; each is worded `file:line: ...`.
pub fn marked_tests(file: &str, text: &str) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let mut tests = Vec::new();
    let mut problems = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    let mut pending_line = 0;
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        let body = line.trim_start_matches(['/', '#', '*', '!']).trim_start();
        if let Some(list) = body.strip_prefix("covers:") {
            if pending.is_empty() {
                pending_line = number;
            }
            for token in list.split([',', ' ', '\t']).filter(|t| !t.is_empty()) {
                if is_id(token) {
                    pending.push(token.to_owned());
                } else {
                    problems.push(format!("{file}:{number}: `{token}` is not a contract id"));
                }
            }
            continue;
        }
        // Blank lines, comments, attributes and decorators keep a pending marker alive.
        let skippable = line.is_empty()
            || line.starts_with("//")
            || line.starts_with("/*")
            || line.starts_with('*')
            || line.starts_with('#')
            || line.starts_with('@');
        if skippable || pending.is_empty() {
            continue;
        }
        match declared_name(line) {
            Some(name) => tests.push((format!("{file}::{name}"), std::mem::take(&mut pending))),
            None => {
                problems.push(format!(
                    "{file}:{pending_line}: covers marker is not followed by a test fn"
                ));
                pending.clear();
            }
        }
    }
    if !pending.is_empty() {
        problems.push(format!(
            "{file}:{pending_line}: covers marker is not followed by a test fn"
        ));
    }
    (tests, problems)
}

/// The name declared by a Rust `fn` or Python `def` line, if the line declares one.
fn declared_name(line: &str) -> Option<String> {
    let mut words = line
        .split_whitespace()
        .skip_while(|w| w.starts_with("pub") || matches!(*w, "async" | "unsafe" | "const"));
    if !matches!(words.next()?, "fn" | "def") {
        return None;
    }
    let word = words.next()?;
    let end = word
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(word.len());
    (end > 0).then(|| word[..end].to_owned())
}

/// The allowlist: `[[exclusion]]` tables with `id` and `reason`, as `(id, reason)` pairs. Every
/// entry needs both keys, an id of contract-id shape and a reason; an id may appear only once.
pub fn parse_exclusions(text: &str) -> Result<Vec<(String, String)>, String> {
    let doc: toml::Table = text
        .parse()
        .map_err(|e| format!("coverage allowlist: {e}"))?;
    let Some(list) = doc.get("exclusion") else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .ok_or("coverage allowlist: `exclusion` must be an array of tables")?;
    let mut out: Vec<(String, String)> = Vec::new();
    for (i, entry) in list.iter().enumerate() {
        let table = entry
            .as_table()
            .ok_or_else(|| format!("exclusion[{i}]: expected a table"))?;
        let need = |key: &str| {
            table
                .get(key)
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("exclusion[{i}]: missing `{key}`"))
        };
        let id = need("id")?;
        let reason = need("reason")?;
        if !is_id(&id) {
            return Err(format!("exclusion[{i}]: `{id}` is not a contract id"));
        }
        if out.iter().any(|(listed, _)| *listed == id) {
            return Err(format!("exclusion[{i}]: `{id}` is listed twice"));
        }
        out.push((id, reason));
    }
    Ok(out)
}

/// The inventory joined with its covering sources and the allowlist.
#[derive(Debug, Default, PartialEq)]
pub struct Coverage {
    /// Every inventory id, with the sources that name it (empty when nothing covers it).
    pub by_id: BTreeMap<String, Vec<String>>,
    /// Inventory ids that no source covers and the allowlist excludes, with the reason.
    pub excluded: BTreeMap<String, String>,
    /// Ids that sources name but the inventory does not list, with the sources naming them.
    pub unknown: BTreeMap<String, Vec<String>>,
    /// Allowlisted ids that the inventory does not list.
    pub unknown_exclusions: Vec<String>,
}

impl Coverage {
    /// Inventory ids that no source covers and the allowlist does not exclude, in id order.
    pub fn uncovered(&self) -> Vec<&str> {
        self.by_id
            .iter()
            .filter(|(id, sources)| sources.is_empty() && !self.excluded.contains_key(*id))
            .map(|(id, _)| id.as_str())
            .collect()
    }
}

/// Join the inventory with `(source name, covers)` pairs and the allowlist. A source counts once
/// per id it names, and lists keep the order of `sources`. An allowlisted id that some source
/// covers counts as covered, so its exclusion is not reported.
pub fn coverage(
    inventory: &[String],
    sources: &[(String, Vec<String>)],
    exclusions: &[(String, String)],
) -> Coverage {
    let mut by_id: BTreeMap<String, Vec<String>> = inventory
        .iter()
        .map(|id| (id.clone(), Vec::new()))
        .collect();
    let mut unknown: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (source, covers) in sources {
        let distinct: BTreeSet<&String> = covers.iter().collect();
        for id in distinct {
            match by_id.get_mut(id) {
                Some(list) => list.push(source.clone()),
                None => unknown.entry(id.clone()).or_default().push(source.clone()),
            }
        }
    }
    let mut excluded = BTreeMap::new();
    let mut unknown_exclusions = Vec::new();
    for (id, reason) in exclusions {
        match by_id.get(id) {
            Some(list) if list.is_empty() => {
                excluded.insert(id.clone(), reason.clone());
            }
            Some(_) => {}
            None => unknown_exclusions.push(id.clone()),
        }
    }
    Coverage {
        by_id,
        excluded,
        unknown,
        unknown_exclusions,
    }
}

/// Human report: the covered ids with their sources, then the excluded ids with their reasons,
/// then the uncovered ids, then any ids the inventory does not list, then the problems found while
/// reading the sources (not counted).
pub fn report_text(c: &Coverage, invalid: &[String]) -> String {
    let total = c.by_id.len();
    let uncovered = c.uncovered();
    let covered = total - uncovered.len() - c.excluded.len();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "coverage: {covered} of {total} contract ids covered, {} excluded with a reason, {} uncovered",
        c.excluded.len(),
        uncovered.len()
    );
    for (id, sources) in c.by_id.iter().filter(|(_, s)| !s.is_empty()) {
        let _ = writeln!(out, "{id}  {}", sources.join(", "));
    }
    if !c.excluded.is_empty() {
        out.push_str("excluded:\n");
        for (id, reason) in &c.excluded {
            let _ = writeln!(out, "  {id}  {reason}");
        }
    }
    if !uncovered.is_empty() {
        out.push_str("uncovered:\n");
        for id in &uncovered {
            let _ = writeln!(out, "  {id}");
        }
    }
    if !c.unknown.is_empty() {
        out.push_str("not in the inventory:\n");
        for (id, sources) in &c.unknown {
            let _ = writeln!(out, "  {id}  {}", sources.join(", "));
        }
    }
    if !c.unknown_exclusions.is_empty() {
        out.push_str("allowlisted but not in the inventory:\n");
        for id in &c.unknown_exclusions {
            let _ = writeln!(out, "  {id}");
        }
    }
    if !invalid.is_empty() {
        out.push_str("invalid (not counted):\n");
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
        "covered": c.by_id.len() - uncovered.len() - c.excluded.len(),
        "excluded": c.excluded,
        "uncovered": uncovered,
        "ids": c.by_id,
        "unknown": c.unknown,
        "unknown_exclusions": c.unknown_exclusions,
        "invalid": invalid,
    })
}
