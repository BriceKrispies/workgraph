//! Tool calls that reach outside a trial's clone: port of `src/workgraph/experiments/escape.py`
//! (CORE-26, GV-17).
//!
//! A trial's solution lives in the repository's own checkout at HEAD, so a subagent that reads
//! that checkout has left its clone. Matching is on text: every string in a tool call's input is
//! normalised (lowercase, forward slashes, MSYS `/c/` drive prefixes turned into `c:/`, repeated
//! slashes collapsed) and checked for a forbidden root followed by the end of the string, a slash,
//! a quote, whitespace, or one of `;`, `&`, `|`.

use serde_json::Value;

/// Longest excerpt kept from the offending input string, in characters (`EXCERPT_CHARS`).
pub const EXCERPT_CHARS: usize = 300;

/// One tool call as the escape check reads it: the tool name, its input and its use id.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    /// Tool name, for example `Bash` or `Read`.
    pub name: String,
    /// The tool's input, a JSON value.
    pub input: Value,
    /// The id of the tool use in the transcript.
    pub tool_use_id: String,
}

/// One (call, root) pair whose input mentions the root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Escape {
    /// Tool name of the offending call.
    pub tool: String,
    /// Tool use id of the offending call.
    pub tool_use_id: String,
    /// The forbidden root as the caller wrote it.
    pub root: String,
    /// The offending input string, cut to [`EXCERPT_CHARS`] characters.
    pub excerpt: String,
}

/// Normalises path text: lowercase, backslashes to slashes, repeated slashes collapsed, and an
/// MSYS drive prefix (`/c/`, not preceded by a word character, colon or slash) turned into `c:/`.
#[must_use]
pub fn normalize_path_text(text: &str) -> String {
    let lowered = text.to_lowercase().replace('\\', "/");
    drive_prefixes(&collapse_slashes(&lowered))
}

fn collapse_slashes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars
        .iter()
        .enumerate()
        .filter(|(i, c)| !(**c == '/' && *i > 0 && chars[i - 1] == '/'))
        .map(|(_, c)| *c)
        .collect()
}

/// Replaces every `/x/` drive prefix (the regex `(?<![\w:/])/([a-z])/` of the Python source).
///
/// A match at `p` consumes `p..p + 3`. A match can never start inside another (the character
/// before a second slash is always the letter of the first match, which is a word character), so
/// each position is either a match start, consumed by the match before it, or copied.
fn drive_prefixes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len())
        .flat_map(|p| {
            if drive_start(&chars, p) {
                vec![chars[p + 1], ':', '/']
            } else if p.checked_sub(1).is_some_and(|k| drive_start(&chars, k))
                || p.checked_sub(2).is_some_and(|k| drive_start(&chars, k))
            {
                Vec::new()
            } else {
                vec![chars[p]]
            }
        })
        .collect()
}

fn drive_start(chars: &[char], i: usize) -> bool {
    let letter_then_slash =
        chars.get(i + 1).is_some_and(char::is_ascii_lowercase) && chars.get(i + 2) == Some(&'/');
    let preceded_ok = match i.checked_sub(1).and_then(|k| chars.get(k)) {
        None => true,
        Some(prev) => !(prev.is_alphanumeric() || *prev == '_' || *prev == ':' || *prev == '/'),
    };
    chars.get(i) == Some(&'/') && letter_then_slash && preceded_ok
}

/// The forbidden roots as `(original, normalised)` pairs: the normalised form has its trailing
/// slashes removed, empty roots are dropped and a repeated original is kept once.
fn forbidden(roots: &[String]) -> Vec<(String, String)> {
    roots
        .iter()
        .map(|root| {
            (
                root.clone(),
                normalize_path_text(root).trim_end_matches('/').to_owned(),
            )
        })
        .filter(|(_, normalized)| !normalized.is_empty())
        .fold(Vec::new(), |accepted, (root, normalized)| {
            if accepted.iter().any(|(seen, _)| *seen == root) {
                accepted
            } else {
                [accepted, vec![(root, normalized)]].concat()
            }
        })
}

/// Whether `text` contains `needle` followed by the end of the string or a root terminator.
fn reaches(text: &str, needle: &str) -> bool {
    text.char_indices()
        .any(|(k, _)| text[k..].starts_with(needle) && root_ends(&text[k + needle.len()..]))
}

fn root_ends(rest: &str) -> bool {
    match rest.chars().next() {
        None => true,
        Some(c) => {
            c == '/' || c == '"' || c == '\'' || c.is_whitespace() || matches!(c, ';' | '&' | '|')
        }
    }
}

/// Every string in a JSON value, depth first in document order. Object keys are not searched,
/// and neither are non-string scalars.
fn strings(value: &Value) -> Vec<&str> {
    match value {
        Value::String(text) => vec![text.as_str()],
        Value::Object(map) => map.values().flat_map(strings).collect(),
        Value::Array(items) => items.iter().flat_map(strings).collect(),
        _ => Vec::new(),
    }
}

/// One [`Escape`] per (call, root) whose input mentions the root. Each call is checked on its own,
/// and the excerpt is the first offending input string, as it was written.
#[must_use]
pub fn find_escapes(calls: &[ToolCall], forbidden_roots: &[String]) -> Vec<Escape> {
    let roots = forbidden(forbidden_roots);
    calls
        .iter()
        .flat_map(|call| {
            let values: Vec<(&str, String)> = strings(&call.input)
                .into_iter()
                .map(|raw| (raw, normalize_path_text(raw)))
                .collect();
            roots
                .iter()
                .filter_map(|(original, needle)| {
                    values
                        .iter()
                        .find(|(_, normalized)| reaches(normalized, needle))
                        .map(|(raw, _)| Escape {
                            tool: call.name.clone(),
                            tool_use_id: call.tool_use_id.clone(),
                            root: original.clone(),
                            excerpt: raw.chars().take(EXCERPT_CHARS).collect(),
                        })
                })
                .collect::<Vec<Escape>>()
        })
        .collect()
}
