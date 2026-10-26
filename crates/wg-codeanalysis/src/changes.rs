//! Change attribution: which functions and types a diff added, modified or deleted.
//!
//! Each changed line is attributed to its innermost symbol in the new (for added lines) or old
//! (for deleted lines) source. Files that are unsupported or do not parse are reported at line
//! level with the reason, by [`FileStructure::fallback`].

use std::collections::{HashMap, HashSet};
use std::fmt::Display;

use crate::lang::Language;
use crate::symbols::{Symbol, SymbolKind};

/// Version of the analysis rules. Recorded with cached results so a rule change invalidates them.
pub const ANALYZER_VERSION: &str = "ts-1";

/// How a symbol changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// Whether a file was analysed structurally or only at line level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Structural,
    Fallback,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Structural => "structural",
            Self::Fallback => "fallback",
        }
    }
}

/// One symbol touched by the diff. `lines_changed` counts the changed lines attributed to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolChange {
    pub kind: SymbolKind,
    pub name: String,
    pub change: ChangeKind,
    pub start_line: usize,
    pub end_line: usize,
    pub lines_changed: usize,
}

/// The analysis of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStructure {
    pub path: String,
    pub language: Option<String>,
    pub mode: Mode,
    pub reason: Option<String>,
    pub changes: Vec<SymbolChange>,
    pub unattributed_lines: usize,
}

impl FileStructure {
    /// A line-level result with no symbols, for a file that cannot be analysed structurally.
    pub fn fallback(path: &str, language: Option<&str>, reason: &str) -> Self {
        Self {
            path: path.to_owned(),
            language: language.map(str::to_owned),
            mode: Mode::Fallback,
            reason: Some(reason.to_owned()),
            changes: Vec::new(),
            unattributed_lines: 0,
        }
    }

    /// The number of distinct function names among the changes.
    pub fn functions(&self) -> usize {
        self.distinct_names(SymbolKind::Function)
    }

    /// The number of distinct type names among the changes.
    pub fn types(&self) -> usize {
        self.distinct_names(SymbolKind::Type)
    }

    fn distinct_names(&self, kind: SymbolKind) -> usize {
        self.changes
            .iter()
            .filter(|change| change.kind == kind)
            .map(|change| change.name.as_str())
            .collect::<HashSet<_>>()
            .len()
    }
}

/// Parses one hunk header `@@ -a[,b] +c[,d] @@` into `(old_start, old_len, new_start, new_len)`.
/// A missing length is 1.
fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_start, old_len, rest) = parse_range(rest)?;
    let rest = rest.strip_prefix(" +")?;
    let (new_start, new_len, rest) = parse_range(rest)?;
    rest.strip_prefix(" @@")?;
    Some((old_start, old_len, new_start, new_len))
}

/// Parses `start[,len]` from the front of `text`, returning the rest of the text.
fn parse_range(text: &str) -> Option<(usize, usize, &str)> {
    let (start, rest) = take_number(text)?;
    match rest.strip_prefix(',') {
        Some(after_comma) => {
            let (len, rest) = take_number(after_comma)?;
            Some((start, len, rest))
        }
        None => Some((start, 1, rest)),
    }
}

fn take_number(text: &str) -> Option<(usize, &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let number = text[..digits].parse().ok()?;
    Some((number, &text[digits..]))
}

/// The old-side deleted line numbers and the new-side added line numbers of a `-U0` diff.
pub fn parse_hunks(diff_text: &str) -> (Vec<usize>, Vec<usize>) {
    let mut deleted = Vec::new();
    let mut added = Vec::new();
    for line in diff_text.lines() {
        let Some((old_start, old_len, new_start, new_len)) = parse_hunk_header(line) else {
            continue;
        };
        deleted.extend(old_start..old_start + old_len);
        added.extend(new_start..new_start + new_len);
    }
    (deleted, added)
}

/// The symbol with the smallest span that contains `line`. On equal spans the later one wins.
fn innermost(symbols: &[Symbol], line: usize) -> Option<&Symbol> {
    let mut best: Option<&Symbol> = None;
    for symbol in symbols {
        if symbol.contains(line) && best.is_none_or(|current| symbol.size() <= current.size()) {
            best = Some(symbol);
        }
    }
    best
}

/// Adds one changed line to `symbol`'s entry, creating the entry on the first line. The first
/// change label recorded for a symbol is kept.
fn record<'a>(
    changes: &mut Vec<SymbolChange>,
    index: &mut HashMap<(SymbolKind, &'a str), usize>,
    symbol: &'a Symbol,
    change: ChangeKind,
) {
    match index.get(&(symbol.kind, symbol.name.as_str())) {
        Some(&position) => changes[position].lines_changed += 1,
        None => {
            index.insert((symbol.kind, symbol.name.as_str()), changes.len());
            changes.push(SymbolChange {
                kind: symbol.kind,
                name: symbol.name.clone(),
                change,
                start_line: symbol.start_line,
                end_line: symbol.end_line,
                lines_changed: 1,
            });
        }
    }
}

/// Attributes the changed lines of one file to its symbols.
///
/// `old_symbols` and `new_symbols` are the symbols of the two sides; a file absent on one side has
/// an empty list there. `language` is the Python-style name of the file's language.
pub fn attribute_changes(
    path: &str,
    language: &str,
    old_symbols: &[Symbol],
    new_symbols: &[Symbol],
    deleted_lines: &[usize],
    added_lines: &[usize],
) -> FileStructure {
    let old_names: HashSet<&str> = old_symbols.iter().map(|s| s.name.as_str()).collect();
    let new_names: HashSet<&str> = new_symbols.iter().map(|s| s.name.as_str()).collect();
    let mut changes: Vec<SymbolChange> = Vec::new();
    let mut index: HashMap<(SymbolKind, &str), usize> = HashMap::new();
    let mut unattributed = 0;

    for &line in added_lines {
        match innermost(new_symbols, line) {
            Some(symbol) => {
                let change = if old_names.contains(symbol.name.as_str()) {
                    ChangeKind::Modified
                } else {
                    ChangeKind::Added
                };
                record(&mut changes, &mut index, symbol, change);
            }
            None => unattributed += 1,
        }
    }
    for &line in deleted_lines {
        match innermost(old_symbols, line) {
            Some(symbol) => {
                let change = if new_names.contains(symbol.name.as_str()) {
                    ChangeKind::Modified
                } else {
                    ChangeKind::Deleted
                };
                record(&mut changes, &mut index, symbol, change);
            }
            None => unattributed += 1,
        }
    }

    FileStructure {
        path: path.to_owned(),
        language: Some(language.to_owned()),
        mode: Mode::Structural,
        reason: None,
        changes,
        unattributed_lines: unattributed,
    }
}

/// Analyses one file. Each present side is extracted with `extract`, which returns `Ok(None)` when
/// a source does not parse and `Err` when a grammar cannot be loaded. A file absent on one side
/// (`None` source) has no symbols there. An unsupported or missing language, an error, or an
/// unparsable side gives a fallback result.
pub fn analyze_file<E: Display>(
    path: &str,
    language: Option<&str>,
    old_source: Option<&[u8]>,
    new_source: Option<&[u8]>,
    deleted_lines: &[usize],
    added_lines: &[usize],
    extract: impl Fn(&[u8], Language) -> Result<Option<Vec<Symbol>>, E>,
) -> FileStructure {
    let Some(lang) = language.and_then(Language::from_name) else {
        return FileStructure::fallback(path, language, "unsupported language");
    };
    let side = |source: Option<&[u8]>| match source {
        Some(bytes) => extract(bytes, lang).map_err(|err| err.to_string()),
        None => Ok(Some(Vec::new())),
    };
    match (side(old_source), side(new_source)) {
        (Err(err), _) | (_, Err(err)) => {
            FileStructure::fallback(path, language, &format!("parse error: {err}"))
        }
        (Ok(Some(old)), Ok(Some(new))) => {
            attribute_changes(path, lang.name(), &old, &new, deleted_lines, added_lines)
        }
        _ => FileStructure::fallback(path, language, "unparsable source"),
    }
}
