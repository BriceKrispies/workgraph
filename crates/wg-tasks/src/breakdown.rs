//! Import of a task breakdown written as a markdown section (`task import-breakdown`), the pure
//! half: `core/services/breakdown.py`.
//!
//! A planning document lists its tasks as bold headers at the start of a line (`**V1. Title**`,
//! `**FD-01 Title**`). A ranged header (`**GV-01 to GV-21 Name**`) is followed by a markdown table
//! whose first column holds one id per task; each row becomes one task titled `Name: <second
//! column>`. The text under a header, up to the next header, is the description. A markdown
//! subheading ends the blocks above it, and a table headed `Id` with no bold header above it is a
//! task table of its own under its subheading's prose. A `Depends on:` or `Deps:` list names other
//! ids (ranges such as `SC-01 to SC-36` expand) or existing task keys; anything else in such a
//! list is reported as an unresolved dependency. `Tier: mid` and `Tier: top` become the tags
//! `tier-mid` and `tier-top`.
//!
//! [`section_lines`] and [`parse_items`] read the document; [`plan`] decides, against the tasks
//! the shell found, what each item needs. The shell applies the plan.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::{Captures, Regex};
use wg_hash::sha256_hex;
use wg_json::py_splitlines;
use wg_types::WgError;

const ID: &str = r"[A-Z][A-Z]*-?\d+";

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|err| unreachable!("breakdown pattern {pattern}: {err}"))
}

static ID_RE: LazyLock<Regex> = LazyLock::new(|| re(&format!("^{ID}$")));
static RANGE_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"^([A-Z][A-Z]*-?)(\d+)\s+to\s+([A-Z][A-Z]*-?)(\d+)$"));
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(#{1,6})\s+(.*?)\s*$"));
static HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"^\*\*({ID})\.?(?:\s+to\s+({ID}))?\s+(.+?)\*\*(.*)$"
    ))
});
static DEPS_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(?:depends on|deps)\s*:\s*(.*)$"));
static TIER_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bTier\s*:\s*(mid|top)\b"));
static TIER_CUT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bTier\s*:"));
static PAREN_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(.*?)\s*\((.*)\)\s*$"));
// Python's `\.(?=\s|$)` has no lookahead here: the dot takes the one space after it, which the
// pieces are trimmed of anyway.
static SEPARATOR_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\s*(?:[,;]|\.(?:\s|$)|\band\b)\s*"));
/// "all IF and SC tasks": every task of the section whose id prefix is one of the listed prefixes.
static ALL_PHRASE_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"\b[Aa]ll\s+([A-Z][A-Z]*(?:\s*(?:,|\band\b|\bor\b)\s*[A-Z][A-Z]*)*)\s+tasks?\b")
});
static PHRASE_MARK_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^\x00(\d+)\x00$"));
static PREFIX_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\s*(?:,|\band\b|\bor\b)\s*"));

const PHRASE_PREFIX: &str = "all-prefixes:";
/// Header names of a table's dependency column.
const DEP_COLUMNS: [&str; 3] = ["deps", "dependencies", "depends on"];
const TIER_TAGS: [&str; 2] = ["tier-mid", "tier-top"];
/// Metadata key: digest of the description the import last wrote. A task whose description no
/// longer matches it was edited by someone else, and a re-run keeps that text.
pub const DESCRIPTION_DIGEST: &str = "breakdown_description_sha256";
/// Cell values of a Deps cell that mean no dependency.
const NO_DEPS: [&str; 3] = ["none", "-", "\u{2014}"];

/// One task of the breakdown, before any write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub description: String,
    pub tier_tags: Vec<String>,
    /// Ids, or free text that is reported unresolved.
    pub deps: Vec<String>,
}

fn norm(text: &str) -> String {
    text.replace("**", " ")
        .replace('#', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// For each line, whether it is inside (or is the edge of) a code fence.
fn fence_flags(lines: &[String]) -> Vec<bool> {
    let mut fenced = false;
    lines
        .iter()
        .map(|line| {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                true
            } else {
                fenced
            }
        })
        .collect()
}

fn heading_of(line: &str, fenced: bool) -> Option<(usize, String)> {
    if fenced {
        return None;
    }
    HEADING_RE
        .captures(line)
        .map(|caps| (caps[1].len(), caps[2].to_owned()))
}

fn lines_of(text: &str) -> Vec<String> {
    py_splitlines(text).into_iter().map(str::to_owned).collect()
}

/// The lines under the heading whose text is `section`, up to the next heading of the same or a
/// higher level.
pub fn section_lines(markdown: &str, section: &str) -> Result<Vec<String>, WgError> {
    let lines = lines_of(markdown);
    let wanted = norm(section);
    let headings: Vec<(usize, usize, String)> = lines
        .iter()
        .zip(fence_flags(&lines))
        .enumerate()
        .filter_map(|(index, (line, fenced))| {
            heading_of(line, fenced).map(|(level, text)| (index, level, text))
        })
        .collect();
    let found: Vec<(usize, usize)> = headings
        .iter()
        .filter(|(_, _, text)| norm(text) == wanted)
        .map(|(index, level, _)| (*index, *level))
        .collect();
    let [(start, level)] = found[..] else {
        return Err(WgError::validation(if found.is_empty() {
            format!("no heading {} in the document", py_repr(section))
        } else {
            format!(
                "the heading {} occurs {} times; rename one of them",
                py_repr(section),
                found.len()
            )
        }));
    };
    let end = headings
        .iter()
        .find(|(index, other, _)| *index > start && *other <= level)
        .map_or(lines.len(), |(index, _, _)| *index);
    Ok(lines[start + 1..end].to_vec())
}

/// Python's `repr()` of a string, for the messages that print one with `!r`.
fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let body: String = text
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".to_owned(),
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            c if c == quote => format!("\\{c}"),
            c => c.to_string(),
        })
        .collect();
    format!("{quote}{body}{quote}")
}

/// Expands `SC-01 to SC-36`; `None` when the text is not a range of ids.
fn expand(token: &str) -> Option<Vec<String>> {
    let caps = RANGE_RE.captures(token)?;
    if caps[1] != caps[3] {
        return None;
    }
    let first: u64 = caps[2].parse().ok()?;
    let last: u64 = caps[4].parse().ok()?;
    if last < first {
        return None;
    }
    let width = caps[2].chars().count();
    Some(
        (first..=last)
            .map(|number| format!("{}{number:0width$}", &caps[1]))
            .collect(),
    )
}

fn prefix_of(task_id: &str) -> &str {
    let end = task_id
        .find(|c: char| !c.is_ascii_uppercase())
        .unwrap_or(task_id.len());
    &task_id[..end]
}

fn tidy(part: &str) -> &str {
    part.trim().trim_end_matches('.').trim()
}

/// The parts of one dependency piece: a parenthetical is kept apart from the text before it.
fn paren_parts(piece: &str) -> Vec<&str> {
    match PAREN_RE.captures(piece) {
        Some(paren) => [paren.get(1), paren.get(2)]
            .into_iter()
            .flatten()
            .map(|m| m.as_str())
            .collect(),
        None => vec![piece],
    }
}

/// Splits the text after `Depends on:` into ids and free text (a parenthetical is its own token).
fn dep_tokens(rest: &str) -> Vec<String> {
    let rest = TIER_CUT_RE.splitn(rest, 2).next().unwrap_or("");
    let mut phrases: Vec<String> = Vec::new();
    let marked = ALL_PHRASE_RE.replace_all(rest, |caps: &Captures<'_>| {
        let prefixes: Vec<&str> = PREFIX_SPLIT_RE
            .split(&caps[1])
            .filter(|p| !p.is_empty())
            .collect();
        phrases.push(format!(
            "{PHRASE_PREFIX}{}\x01{}",
            prefixes.join(","),
            &caps[0]
        ));
        format!(" \x00{}\x00 ", phrases.len() - 1)
    });
    let mut tokens = Vec::new();
    for raw in SEPARATOR_RE.split(&marked) {
        let piece = tidy(raw);
        if let Some(mark) = PHRASE_MARK_RE.captures(piece) {
            let phrase = mark[1].parse::<usize>().ok().and_then(|n| phrases.get(n));
            tokens.extend(phrase.cloned());
            continue;
        }
        if piece.is_empty() || NO_DEPS.contains(&piece.to_lowercase().as_str()) {
            continue;
        }
        for part in paren_parts(piece)
            .into_iter()
            .map(tidy)
            .filter(|p| !p.is_empty())
        {
            match expand(part) {
                Some(ids) => tokens.extend(ids),
                None => tokens.push(part.to_owned()),
            }
        }
    }
    tokens
}

/// Joins a line that opens a parenthesis with the following lines until it closes.
fn joined(lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut pending = String::new();
    for line in lines {
        pending = if pending.is_empty() {
            line.clone()
        } else {
            format!("{pending} {line}").trim().to_owned()
        };
        if pending.matches('(').count() <= pending.matches(')').count() {
            out.push(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        out.push(pending);
    }
    out
}

fn unique<I: IntoIterator<Item = String>>(values: I) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

/// The dependency tokens and the tier tags named by the given lines.
fn text_fields(lines: &[String]) -> (Vec<String>, Vec<String>) {
    let mut deps = Vec::new();
    let mut tags = Vec::new();
    for line in joined(lines) {
        if let Some(caps) = DEPS_RE.captures(&line) {
            deps.extend(dep_tokens(&caps[1]));
        }
        tags.extend(
            TIER_RE
                .captures_iter(&line)
                .map(|tier| format!("tier-{}", tier[1].to_lowercase())),
        );
    }
    (unique(deps), unique(tags))
}

fn cells_of(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

type Columns = HashMap<String, usize>;

struct TableRow {
    line: String,
    cells: Vec<String>,
    columns: Columns,
}

/// The id rows of a block's tables, each with the column index of the header above it. A header
/// is the first non-rule line that does not start with an id; a table that follows another table
/// gets its own header, so its Deps and Tier columns are its own.
fn table_rows(table: &[String]) -> Vec<TableRow> {
    let mut rows = Vec::new();
    let mut columns = Columns::new();
    for line in table {
        let cells = cells_of(line);
        if ID_RE.is_match(&cells[0]) {
            rows.push(TableRow {
                line: line.clone(),
                cells,
                columns: columns.clone(),
            });
        } else if !cells[0].chars().all(|c| "-: ".contains(c)) {
            columns = cells
                .iter()
                .enumerate()
                .map(|(index, cell)| (cell.to_lowercase(), index))
                .collect();
        }
    }
    rows
}

/// The dependency and tier values of one table row. Only the column named as a dependency column
/// counts.
fn row_fields(cells: &[String], columns: &Columns) -> (Vec<String>, Vec<String>) {
    let deps = DEP_COLUMNS
        .iter()
        .find_map(|name| columns.get(*name))
        .and_then(|index| cells.get(*index))
        .map(|cell| dep_tokens(cell))
        .unwrap_or_default();
    let tiers = columns
        .get("tier")
        .and_then(|index| cells.get(*index))
        .map(|cell| cell.to_lowercase())
        .filter(|cell| cell == "mid" || cell == "top")
        .map(|cell| format!("tier-{cell}"))
        .into_iter()
        .collect();
    (deps, tiers)
}

struct Block {
    id: String,
    range_end: Option<String>,
    name: String,
    body: Vec<String>,
}

/// Groups the section's lines under their bold headers.
fn blocks_of(lines: &[String]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for (line, fenced) in lines.iter().zip(fence_flags(lines)) {
        let header = if fenced {
            None
        } else {
            HEADER_RE.captures(line)
        };
        if let Some(header) = header {
            // Text after the closing ** of a header (e.g. "(each: ...)") opens the block's body.
            let trailing = header[4].trim();
            blocks.push(Block {
                id: header[1].to_owned(),
                range_end: header.get(2).map(|m| m.as_str().to_owned()),
                name: header[3].to_owned(),
                body: if trailing.is_empty() {
                    Vec::new()
                } else {
                    vec![trailing.to_owned()]
                },
            });
        } else if let Some(last) = blocks.last_mut() {
            last.body.push(line.clone());
        }
    }
    blocks
}

/// Splits a section at its markdown subheadings: (heading text, lines under it up to the next
/// one). The first part holds the lines before any subheading, with an empty heading.
fn subsections(lines: &[String]) -> Vec<(String, Vec<String>)> {
    let mut parts: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];
    for (line, fenced) in lines.iter().zip(fence_flags(lines)) {
        match heading_of(line, fenced) {
            Some((_, text)) => parts.push((text, Vec::new())),
            None => {
                if let Some(last) = parts.last_mut() {
                    last.1.push(line.clone());
                }
            }
        }
    }
    parts
}

/// The lines of a subsection before its first bold header (outside code fences).
fn lead_of(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .zip(fence_flags(lines))
        .take_while(|(line, fenced)| *fenced || !HEADER_RE.is_match(line))
        .map(|(line, _)| line.clone())
        .collect()
}

fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// One item per table row: the row's own cells, under the prose of the subsection that holds it.
fn row_items(
    rows: &[TableRow],
    prose: &str,
    title_prefix: &str,
    deps: &[String],
    tiers: &[String],
) -> Vec<Item> {
    rows.iter()
        .map(|row| {
            let name = row.cells.get(1).map_or("", String::as_str);
            // A "Deps" or "Tier" column, when the table has one, applies to its row only.
            let (row_deps, row_tiers) = row_fields(&row.cells, &row.columns);
            let title = if !title_prefix.is_empty() && !name.is_empty() {
                format!("{title_prefix}: {name}")
            } else if name.is_empty() {
                title_prefix.to_owned()
            } else {
                name.to_owned()
            };
            Item {
                id: row.cells[0].clone(),
                title,
                description: format!("{prose}\n\n{}", row.line.trim()).trim().to_owned(),
                tier_tags: unique(tiers.iter().cloned().chain(row_tiers)),
                deps: unique(deps.iter().cloned().chain(row_deps)),
            }
        })
        .collect()
}

/// Replaces each "all X and Y tasks" dependency with the section's ids having those prefixes. A
/// phrase that matches no id of the section stays as free text, so it is reported as unresolved.
fn expand_phrases(items: &mut [Item]) {
    let ids: Vec<String> = items.iter().map(|item| item.id.clone()).collect();
    for item in items {
        let mut resolved = Vec::new();
        for token in &item.deps {
            let Some(rest) = token.strip_prefix(PHRASE_PREFIX) else {
                resolved.push(token.clone());
                continue;
            };
            let (prefixes, text) = rest.split_once('\x01').unwrap_or((rest, ""));
            let wanted: HashSet<&str> = prefixes.split(',').collect();
            let matches: Vec<String> = ids
                .iter()
                .filter(|other| **other != item.id && wanted.contains(prefix_of(other)))
                .cloned()
                .collect();
            if matches.is_empty() {
                resolved.push(text.to_owned());
            } else {
                resolved.extend(matches);
            }
        }
        item.deps = unique(resolved);
    }
}

/// Turns the lines of one section into the tasks it describes, in document order.
pub fn parse_items(lines: &[String]) -> Result<Vec<Item>, WgError> {
    let mut items: Vec<Item> = Vec::new();
    for (heading, body) in subsections(lines) {
        // A table before the subsection's first bold header is a task table of its own. Only a
        // table headed "Id" is a task table; a summary table (such as the level counts) is not.
        // The section's own preamble, above any subheading, is shared prose of no subsection.
        let lead = lead_of(&body);
        let lead_tables: Vec<String> = lead.iter().filter(|l| is_table_line(l)).cloned().collect();
        let lead_rows: Vec<TableRow> = table_rows(&lead_tables)
            .into_iter()
            .filter(|row| row.columns.get("id") == Some(&0))
            .collect();
        let shared = if heading.is_empty() {
            String::new()
        } else {
            let prose: Vec<&str> = lead
                .iter()
                .filter(|l| !is_table_line(l))
                .map(String::as_str)
                .collect();
            prose.join("\n").trim().to_owned()
        };
        if !lead_rows.is_empty() {
            let (deps, tiers) = text_fields(&lines_of(&shared));
            items.extend(row_items(&lead_rows, &shared, &heading, &deps, &tiers));
        }
        for block in blocks_of(&body) {
            let Some(range_end) = &block.range_end else {
                let (deps, tiers) = text_fields(&block.body);
                items.push(Item {
                    id: block.id,
                    title: block.name,
                    description: block.body.join("\n").trim().to_owned(),
                    tier_tags: tiers,
                    deps,
                });
                continue;
            };
            let table: Vec<String> = block
                .body
                .iter()
                .filter(|l| is_table_line(l))
                .cloned()
                .collect();
            let block_prose = block
                .body
                .iter()
                .filter(|l| !is_table_line(l))
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            let prose = [shared.as_str(), block_prose.trim()]
                .into_iter()
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            let (deps, tiers) = text_fields(&lines_of(&prose));
            let rows = table_rows(&table);
            if rows.is_empty() {
                return Err(WgError::validation(format!(
                    "ranged block {} to {} ({}) has no table rows",
                    block.id, range_end, block.name
                )));
            }
            items.extend(row_items(&rows, &prose, &block.name, &deps, &tiers));
        }
    }
    expand_phrases(&mut items);
    let mut seen: HashSet<&str> = HashSet::new();
    for item in &items {
        if !seen.insert(&item.id) {
            return Err(WgError::validation(format!(
                "breakdown id {} appears more than once in the section",
                item.id
            )));
        }
    }
    Ok(items)
}

/// Whether `token` is shaped like a task id of a breakdown.
#[must_use]
pub fn is_id(token: &str) -> bool {
    ID_RE.is_match(token)
}

/// A task the shell found: one already imported from this section, a dependency named by key, or
/// the parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Existing {
    pub id: String,
    pub key: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub tags: Vec<String>,
    pub parent_id: Option<String>,
    pub depends_on: Vec<String>,
    /// The digest of the description the import last wrote, when the task records one.
    pub digest: Option<String>,
}

/// What a plan reads of the world: the items, the tasks already imported from this section by
/// breakdown id, the dependency tokens that are ids outside the section (`None` when no such
/// task), the parent as given and as found, and the extra tags.
pub struct PlanInput<'a> {
    pub items: &'a [Item],
    pub existing: &'a HashMap<String, Existing>,
    pub lookups: &'a HashMap<String, Option<Existing>>,
    /// The parent reference as given, and the id of the task it names.
    pub parent: Option<(&'a str, &'a str)>,
    pub tags: &'a [String],
}

/// A dependency of a planned task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    /// Another item of the section, by breakdown id.
    Section(String),
    /// A task of the workspace named by key.
    Task(Existing),
}

/// The fields of an existing task the import changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub title: Option<String>,
    pub description: Option<String>,
    /// The digest to record in the metadata.
    pub digest: Option<String>,
    /// The parent, as given.
    pub parent: Option<String>,
}

/// What the import does with one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub item: Item,
    pub current: Option<Existing>,
    /// `created`, `updated` or `unchanged`.
    pub action: &'static str,
    pub desired_tags: Vec<String>,
    pub changes: Changes,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub deps_to_add: Vec<Ref>,
    /// The status after the import.
    pub status: String,
    /// The task key: the existing one, or the new one once the shell created it.
    pub key: Option<String>,
}

/// A dependency that could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub id: String,
    pub dependency: String,
    pub reason: &'static str,
}

/// A description kept because it was edited since the import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub id: String,
    pub key: String,
    pub reason: &'static str,
}

/// The whole plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub entries: Vec<Entry>,
    /// The dependencies of each entry, in item order.
    pub refs: Vec<Vec<Ref>>,
    pub unresolved: Vec<Unresolved>,
    pub kept: Vec<Kept>,
}

/// The dependency tokens that are ids outside the section: the ones the shell looks up.
#[must_use]
pub fn external_ids(items: &[Item]) -> Vec<String> {
    let ids: HashSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
    let tokens = items.iter().flat_map(|item| item.deps.iter());
    unique(
        tokens
            .filter(|token| is_id(token) && !ids.contains(token.as_str()))
            .cloned(),
    )
}

fn digest(text: &str) -> String {
    sha256_hex(text.as_bytes())
}

/// The digest recorded for a task's description.
#[must_use]
pub fn description_digest(description: &str) -> String {
    digest(description)
}

fn resolve(input: &PlanInput<'_>) -> (Vec<Vec<Ref>>, Vec<Unresolved>) {
    let ids: HashSet<&str> = input.items.iter().map(|item| item.id.as_str()).collect();
    let mut unresolved = Vec::new();
    let mut all_refs: Vec<Vec<Ref>> = Vec::new();
    for item in input.items {
        let mut found = Vec::new();
        for token in &item.deps {
            if !is_id(token) {
                unresolved.push(Unresolved {
                    id: item.id.clone(),
                    dependency: token.clone(),
                    reason: "not a task id",
                });
            } else if *token == item.id {
                // A ranged block's shared "Deps:" may name its own range; that is not a dependency.
            } else if ids.contains(token.as_str()) {
                found.push(Ref::Section(token.clone()));
            } else if let Some(Some(target)) = input.lookups.get(token) {
                found.push(Ref::Task(target.clone()));
            } else {
                unresolved.push(Unresolved {
                    id: item.id.clone(),
                    dependency: token.clone(),
                    reason: "no such task in this section or the workspace",
                });
            }
        }
        all_refs.push(found);
    }
    (all_refs, unresolved)
}

/// Plans each item against the tasks found.
#[must_use]
pub fn plan(input: &PlanInput<'_>) -> Plan {
    let (all_refs, unresolved) = resolve(input);
    let accepted = |r: &Ref| match r {
        Ref::Section(id) => input
            .existing
            .get(id)
            .is_some_and(|t| t.status == "accepted"),
        Ref::Task(target) => target.status == "accepted",
    };
    let mut entries = Vec::new();
    let mut kept = Vec::new();
    for (item, refs) in input.items.iter().zip(&all_refs) {
        let desired = unique(
            input
                .tags
                .iter()
                .cloned()
                .chain(item.tier_tags.iter().cloned()),
        );
        let ready = !unresolved.iter().any(|u| u.id == item.id) && refs.iter().all(accepted);
        let entry = match input.existing.get(&item.id) {
            None => Entry {
                item: item.clone(),
                current: None,
                action: "created",
                desired_tags: desired,
                changes: Changes::default(),
                add_tags: Vec::new(),
                remove_tags: Vec::new(),
                deps_to_add: refs.clone(),
                status: if ready { "ready" } else { "backlog" }.to_owned(),
                key: None,
            },
            Some(current) => plan_existing(input, item, refs, current, desired, ready, &mut kept),
        };
        entries.push(entry);
    }
    Plan {
        entries,
        refs: all_refs,
        unresolved,
        kept,
    }
}

fn plan_existing(
    input: &PlanInput<'_>,
    item: &Item,
    refs: &[Ref],
    current: &Existing,
    desired: Vec<String>,
    ready: bool,
    kept: &mut Vec<Kept>,
) -> Entry {
    let present = |r: &Ref| match r {
        Ref::Section(id) => input
            .existing
            .get(id)
            .is_some_and(|target| current.depends_on.contains(&target.id)),
        Ref::Task(target) => current.depends_on.contains(&target.id),
    };
    let mut changes = Changes::default();
    if current.title != item.title {
        changes.title = Some(item.title.clone());
    }
    let wanted = digest(&item.description);
    if current.description == item.description {
        if current.digest.as_deref() != Some(wanted.as_str()) {
            changes.digest = Some(wanted); // adopt the import record
        }
    } else if current.digest.as_deref() == Some(digest(&current.description).as_str()) {
        // Unchanged since the last import: the document's text replaces it.
        changes.description = Some(item.description.clone());
        changes.digest = Some(wanted);
    } else {
        // Edited since the import, or imported before records existed: keep it and report it.
        kept.push(Kept {
            id: item.id.clone(),
            key: current.key.clone(),
            reason: "description edited since import",
        });
    }
    if let Some((given, parent_id)) = input.parent
        && current.parent_id.as_deref() != Some(parent_id)
    {
        changes.parent = Some(given.to_owned());
    }
    let add_tags: Vec<String> = desired
        .iter()
        .filter(|t| !current.tags.contains(t))
        .cloned()
        .collect();
    let remove_tags: Vec<String> = current
        .tags
        .iter()
        .filter(|t| TIER_TAGS.contains(&t.as_str()) && !desired.contains(t))
        .cloned()
        .collect();
    let deps_to_add: Vec<Ref> = refs.iter().filter(|r| !present(r)).cloned().collect();
    let status = if current.status == "backlog" && ready {
        "ready".to_owned()
    } else {
        current.status.clone()
    };
    let changed = changes != Changes::default()
        || !add_tags.is_empty()
        || !remove_tags.is_empty()
        || !deps_to_add.is_empty()
        || status != current.status;
    Entry {
        item: item.clone(),
        current: Some(current.clone()),
        action: if changed { "updated" } else { "unchanged" },
        desired_tags: desired,
        changes,
        add_tags,
        remove_tags,
        deps_to_add,
        status,
        key: Some(current.key.clone()),
    }
}

/// The key a dependency resolves to, given the keys the entries have.
#[must_use]
pub fn ref_key(plan: &Plan, r: &Ref) -> Option<String> {
    match r {
        Ref::Section(id) => plan
            .entries
            .iter()
            .find(|entry| entry.item.id == *id)
            .and_then(|entry| entry.key.clone()),
        Ref::Task(target) => Some(target.key.clone()),
    }
}

/// Whether an entry is promoted to ready at the end of the apply.
#[must_use]
pub fn promotes(entry: &Entry) -> bool {
    entry.status == "ready"
        && entry
            .current
            .as_ref()
            .is_none_or(|current| current.status == "backlog")
}

/// Whether an existing entry has fields to update.
#[must_use]
pub fn needs_update(entry: &Entry) -> bool {
    entry.current.is_some()
        && (entry.changes != Changes::default()
            || !entry.add_tags.is_empty()
            || !entry.remove_tags.is_empty())
}
