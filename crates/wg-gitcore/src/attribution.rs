//! Commit-to-task attribution from what a commit itself says: its `Task-ID:`-style trailers and
//! the task keys its message mentions. `golden/attribution/` (GV-04) pins the results.
//!
//! Explicit links, execution contexts and worktree reachability are decided by the callers that
//! hold that state. Each candidate records its source, confidence and the evidence it rests on.

use std::cmp::Reverse;
use std::collections::BTreeSet;

use regex::Regex;

use crate::Trailer;

/// Confidence of a candidate read from a `Task-ID:`-style trailer.
pub const TRAILER_CONFIDENCE: f64 = 0.95;
/// Confidence of a message candidate when the message names exactly one key.
pub const MESSAGE_CONFIDENCE: f64 = 0.5;
/// Confidence of every message candidate when the message names several keys.
pub const AMBIGUOUS_MESSAGE_CONFIDENCE: f64 = 0.35;
/// The label recorded in the evidence of every message candidate.
pub const MESSAGE_LABEL: &str = "inferred from commit message";

/// Trailer keys that name a task, compared lower-cased.
const TASK_TRAILER_KEYS: [&str; 3] = ["task-id", "workgraph-task", "wg-task"];

/// Where a candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSource {
    Trailer,
    MessageInference,
}

impl CandidateSource {
    /// The name the attribution records and the golden vectors use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trailer => "trailer",
            Self::MessageInference => "message_inference",
        }
    }
}

/// Why a candidate was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    /// The trailer as `Key: value`.
    Trailer { trailer: String },
    /// The key as matched, whether the message named several keys, and every distinct key it
    /// named in order of appearance.
    Message {
        label: &'static str,
        matched: String,
        ambiguous: bool,
        all_matches: Vec<String>,
    },
}

/// One task a commit may belong to.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub task_ref: String,
    pub source: CandidateSource,
    pub confidence: f64,
    pub evidence: Evidence,
}

/// Task references in trailers of a task-naming key. The value is split on commas and
/// whitespace, so one trailer may name several tasks.
pub fn trailer_candidates(trailers: &[Trailer]) -> Vec<Candidate> {
    trailers
        .iter()
        .filter(|trailer| TASK_TRAILER_KEYS.contains(&trailer.key.to_lowercase().as_str()))
        .flat_map(|trailer| {
            trailer
                .value
                .split(|c: char| c == ',' || is_py_space(c))
                .filter(|value| !value.is_empty())
                .map(move |value| Candidate {
                    task_ref: value.to_owned(),
                    source: CandidateSource::Trailer,
                    confidence: TRAILER_CONFIDENCE,
                    evidence: Evidence::Trailer {
                        trailer: format!("{}: {}", trailer.key, trailer.value),
                    },
                })
        })
        .collect()
}

/// Task keys mentioned in the prose of a message, for any of the given prefixes.
///
/// Keys are normalised to `PREFIX-N` in upper case. The trailer lines of the message are not
/// prose and are skipped. Several distinct keys lower every candidate's confidence.
pub fn message_candidates(
    message: &str,
    key_prefixes: &[&str],
    trailers: &[Trailer],
) -> Vec<Candidate> {
    // Distinct, longest first, so a longer prefix is tried before a shorter one that it starts
    // with; ties keep lexicographic order.
    let prefixes: Vec<&str> = key_prefixes
        .iter()
        .copied()
        .filter(|prefix| !prefix.is_empty())
        .map(|prefix| (Reverse(prefix.chars().count()), prefix))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|(_, prefix)| prefix)
        .collect();
    if prefixes.is_empty() {
        return Vec::new();
    }

    let trailer_lines: Vec<String> = trailers
        .iter()
        .map(|t| format!("{}: {}", t.key, t.value))
        .collect();
    let prose = split_lines(message)
        .into_iter()
        .filter(|line| !trailer_lines.iter().any(|t| t == trim_py(line)))
        .collect::<Vec<_>>()
        .join("\n");

    let alternation = prefixes
        .iter()
        .map(|prefix| regex::escape(prefix))
        .collect::<Vec<_>>()
        .join("|");
    // Compiled where it is used: a `static` lazy regex would be interior mutability in a Core crate.
    let Ok(pattern) = Regex::new(&format!("(?i)({alternation})-(\\d+)")) else {
        return Vec::new();
    };

    let keys = find_keys(&pattern, &prose);
    if keys.is_empty() {
        return Vec::new();
    }
    let ambiguous = keys.len() > 1;
    let confidence = if ambiguous {
        AMBIGUOUS_MESSAGE_CONFIDENCE
    } else {
        MESSAGE_CONFIDENCE
    };
    keys.iter()
        .map(|key| Candidate {
            task_ref: key.clone(),
            source: CandidateSource::MessageInference,
            confidence,
            evidence: Evidence::Message {
                label: MESSAGE_LABEL,
                matched: key.clone(),
                ambiguous,
                all_matches: keys.clone(),
            },
        })
        .collect()
}

/// Every key the pattern finds in `prose`, normalised and de-duplicated in order of appearance.
///
/// A match counts only when the character before it is not an ASCII letter or digit (the rule
/// the original lookbehind expressed). The regex crate has no lookaround, so the check runs here
/// and a rejected match resumes the search one character after its start.
fn find_keys(pattern: &Regex, prose: &str) -> Vec<String> {
    // Each step is one match attempt from `pos`: the key it accepted, if any, and where the next
    // attempt starts. The first item is only the starting point and carries no key.
    let steps = std::iter::successors(Some((0_usize, None::<String>)), |(pos, _)| {
        scan_step(pattern, prose, *pos)
    });
    let keys: Vec<String> = steps.filter_map(|(_, key)| key).collect();
    keys.iter()
        .enumerate()
        .filter(|(index, key)| !keys[..*index].contains(key))
        .map(|(_, key)| key.clone())
        .collect()
}

/// One match attempt of [`find_keys`] from `pos`. `None` when no further match exists.
fn scan_step(pattern: &Regex, prose: &str, pos: usize) -> Option<(usize, Option<String>)> {
    let caps = pattern.captures_at(prose, pos)?;
    let whole = caps.get(0)?;
    let (start, end) = (whole.start(), whole.end());
    let preceded_by_alnum = prose[..start]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    let followed_by_digit = prose[end..].starts_with(|c: char| c.is_ascii_digit());
    if preceded_by_alnum || followed_by_digit {
        let step = prose[start..].chars().next().map_or(1, char::len_utf8);
        return Some((start + step, None));
    }
    let key = match (caps.get(1), caps.get(2)) {
        (Some(prefix), Some(digits)) => Some(format!(
            "{}-{}",
            prefix.as_str().to_uppercase(),
            digits.as_str()
        )),
        _ => None,
    };
    Some((end, key))
}

/// The lines of `text` as Python's `str.splitlines` cuts them: `\r\n`, `\n`, `\r`, and the other
/// line boundaries Python recognises. The terminators are dropped and a final empty line is not
/// returned.
fn split_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    // Each cut is the index where a line ends and the index where the next line starts. A `\n`
    // that follows a `\r` is part of that `\r\n` cut, so it does not cut again.
    let cuts: Vec<(usize, usize)> = text
        .char_indices()
        .filter(|&(index, c)| {
            let boundary = matches!(
                c,
                '\n' | '\r'
                    | '\x0b'
                    | '\x0c'
                    | '\x1c'
                    | '\x1d'
                    | '\x1e'
                    | '\u{85}'
                    | '\u{2028}'
                    | '\u{2029}'
            );
            boundary && !(c == '\n' && index > 0 && bytes[index - 1] == b'\r')
        })
        .map(|(index, c)| {
            let next = if c == '\r' && bytes.get(index + 1) == Some(&b'\n') {
                index + 2
            } else {
                index + c.len_utf8()
            };
            (index, next)
        })
        .collect();
    let starts = std::iter::once(0).chain(cuts.iter().map(|&(_, next)| next));
    let ends = cuts
        .iter()
        .map(|&(index, _)| index)
        .chain(std::iter::once(text.len()));
    let segment_count = cuts.len() + 1;
    starts
        .zip(ends)
        .map(|(start, end)| &text[start..end])
        .enumerate()
        // The text after the last cut is a line only when it is not empty.
        .filter(|(index, line)| index + 1 < segment_count || !line.is_empty())
        .map(|(_, line)| line)
        .collect()
}

/// Python's `str.isspace` for one character: Unicode white space plus the file, group, record
/// and unit separators.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Python's `str.strip()`.
fn trim_py(text: &str) -> &str {
    text.trim_matches(is_py_space)
}
