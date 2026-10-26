//! The full normaliser of section 3.4 of `docs/rust-rewrite.md`.
//!
//! Values that differ between two runs of one scenario are rewritten into
//! placeholders: UUIDs, timestamps, git SHAs, durations, ports and pids. The
//! numbering is per run, so a value gets the same placeholder everywhere in one
//! run and structure and cross-references still compare.
//!
//! The trait method sees one output text at a time, so every call rebuilds the
//! run's tables from all of its outputs. The result depends only on the run and
//! the text, which lets one normaliser serve both sides of a diff.

use crate::compare::{Normaliser, RootsNormaliser};
use crate::runner::RunResult;
use std::collections::{BTreeMap, BTreeSet};

/// The section 3.4 normaliser. `literals` are values the runner knows (the API
/// token, the pairing code, ...) and the placeholder each one becomes.
#[derive(Debug, Clone, Default)]
pub struct FullNormaliser {
    pub literals: Vec<(String, String)>,
}

impl Normaliser for FullNormaliser {
    fn normalise(&self, run: &RunResult, text: &str) -> String {
        // Line endings: Python writes CRLF to redirected Windows stdout, Rust writes LF. No
        // consumer parses the terminator, so both compare as LF (DV10).
        let mut out = RootsNormaliser.normalise(run, text).replace("\r\n", "\n");
        for (value, placeholder) in &self.literals {
            if !value.is_empty() {
                out = out.replace(value.as_str(), placeholder);
            }
        }
        let outputs: Vec<&str> = run
            .steps
            .iter()
            .flat_map(|s| [s.stdout.as_str(), s.stderr.as_str()])
            .chain(std::iter::once(text))
            .collect();
        let mut numbering = Numbering::from_outputs(&outputs);
        let bytes = out.as_bytes();
        let clusters = instant_clusters(&stamps_of(text));

        let mut spans: Vec<(usize, usize, String)> = Vec::new();
        for (s, e, key) in scan(bytes, uuid_at) {
            spans.push((s, e, format!("<id:{}>", numbering.id(&key))));
        }
        for (s, e, prefix) in scan(bytes, attempt_prefix_at) {
            spans.push((s, e, format!("<id:{}>", numbering.attempt(&prefix))));
        }
        for (s, e, key) in scan(bytes, sha_at) {
            spans.push((s, e, format!("<sha:{}>", numbering.sha(&key))));
        }
        for (s, e, stamp) in scan(bytes, timestamp_at) {
            let rank = numbering.rank(stamp.micros, &clusters);
            spans.push((s, e, format!("<ts:{rank}|{}>", stamp.format)));
        }
        for (s, e, worker) in scan(bytes, worker_at) {
            spans.push((s, e, format!("<worker:{}>", numbering.worker(&worker))));
        }
        for (s, e, placeholder) in scan(bytes, json_field_at) {
            spans.push((s, e, placeholder.to_owned()));
        }
        for (s, e, ()) in scan(bytes, local_port_at) {
            spans.push((s, e, "<port>".to_owned()));
        }
        sort_set_lists(&splice(&out, spans))
    }
}

/// Byte span `(start, end)` with a payload.
type Span<T> = (usize, usize, T);

/// Every non-overlapping match of `at`, scanning left to right.
fn scan<T>(bytes: &[u8], mut at: impl FnMut(&[u8], usize) -> Option<Span<T>>) -> Vec<Span<T>> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match at(bytes, i) {
            Some((s, e, value)) => {
                found.push((s, e, value));
                i = e.max(i + 1);
            }
            None => i += 1,
        }
    }
    found
}

/// Replace each span. Spans come from byte-level matches over ASCII, so they
/// always start and end on character boundaries. Overlaps keep the first span.
fn splice(text: &str, mut spans: Vec<(usize, usize, String)>) -> String {
    spans.sort_by_key(|(s, _, _)| *s);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e, replacement) in spans {
        if s < last {
            continue;
        }
        out.push_str(&text[last..s]);
        out.push_str(&replacement);
        last = e;
    }
    out.push_str(&text[last..]);
    out
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn bounded(bytes: &[u8], start: usize, end: usize) -> bool {
    (start == 0 || !is_word(bytes[start - 1])) && (end >= bytes.len() || !is_word(bytes[end]))
}

fn uuid_at(bytes: &[u8], i: usize) -> Option<Span<String>> {
    let end = i + 36;
    if end > bytes.len() || !bounded(bytes, i, end) {
        return None;
    }
    let shape = (0..36).all(|k| match k {
        8 | 13 | 18 | 23 => bytes[i + k] == b'-',
        _ => bytes[i + k].is_ascii_hexdigit(),
    });
    shape.then(|| (i, end, lower(&bytes[i..end])))
}

/// The 8-character prefix of an attempt id in a managed branch name, `attempt-<n>-<id8>`
/// (`wg/<key>/attempt-<n>-<id8>`). Only the prefix is the span. It takes the placeholder of the
/// full UUID it prefixes, so the branch name and the attempt's `id` agree.
fn attempt_prefix_at(bytes: &[u8], i: usize) -> Option<Span<String>> {
    const HEAD: &[u8] = b"attempt-";
    if !bytes[i..].starts_with(HEAD) || (i > 0 && !matches!(bytes[i - 1], b'/' | b'-' | b'_')) {
        return None;
    }
    let mut p = i + HEAD.len();
    let digits_from = p;
    while bytes.get(p).is_some_and(u8::is_ascii_digit) {
        p += 1;
    }
    if p == digits_from || bytes.get(p) != Some(&b'-') {
        return None;
    }
    let start = p + 1;
    let end = start + 8;
    if end > bytes.len() || !bounded(bytes, start, end) {
        return None;
    }
    bytes[start..end]
        .iter()
        .all(u8::is_ascii_hexdigit)
        .then(|| (start, end, lower(&bytes[start..end])))
}

fn sha_at(bytes: &[u8], i: usize) -> Option<Span<String>> {
    let end = i + 40;
    if end > bytes.len() || !bounded(bytes, i, end) {
        return None;
    }
    bytes[i..end]
        .iter()
        .all(u8::is_ascii_hexdigit)
        .then(|| (i, end, lower(&bytes[i..end])))
}

fn lower(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_ascii_lowercase()
}

/// A timestamp in one of the formats of O01 and O11, with its instant in
/// microseconds since the epoch and the format it was written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    micros: i64,
    format: &'static str,
}

/// `YYYY-MM-DDTHH:MM:SS[.f+][Z|+00:00]` and `YYYY-MM-DD HH:MM:SS[.f+]` (display).
/// The format is `Z`, `+00:00`, `db` (O11: six fraction digits with `+00:00`) or
/// `display` (space separator, no offset).
fn timestamp_at(bytes: &[u8], i: usize) -> Option<Span<Stamp>> {
    if (i > 0 && bytes[i - 1].is_ascii_digit()) || i + 19 > bytes.len() {
        return None;
    }
    let num = |at: usize, n: usize| -> Option<i64> {
        let part = bytes.get(at..at + n)?;
        part.iter().all(u8::is_ascii_digit).then(|| {
            part.iter()
                .fold(0_i64, |acc, d| acc * 10 + i64::from(d - b'0'))
        })
    };
    let year = num(i, 4)?;
    let month = num(i + 5, 2)?;
    let day = num(i + 8, 2)?;
    let sep = bytes[i + 10];
    let hour = num(i + 11, 2)?;
    let minute = num(i + 14, 2)?;
    let second = num(i + 17, 2)?;
    let shaped = bytes[i + 4] == b'-'
        && bytes[i + 7] == b'-'
        && (sep == b'T' || sep == b' ')
        && bytes[i + 13] == b':'
        && bytes[i + 16] == b':';
    if !shaped
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut pos = i + 19;
    let mut frac_digits = 0;
    let mut frac_micros = 0_i64;
    if bytes.get(pos) == Some(&b'.') {
        let start = pos + 1;
        let mut end = start;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            if end - start < 6 {
                frac_micros = frac_micros * 10 + i64::from(bytes[end] - b'0');
            }
            end += 1;
        }
        if end == start {
            return None;
        }
        frac_digits = end - start;
        for _ in frac_digits.min(6)..6 {
            frac_micros *= 10;
        }
        pos = end;
    }
    let offset = if bytes.get(pos) == Some(&b'Z') {
        pos += 1;
        Some("Z")
    } else if bytes.get(pos..pos + 6) == Some(b"+00:00") {
        pos += 6;
        Some("+00:00")
    } else {
        None
    };
    let format = match (sep, offset) {
        (b'T', Some("Z")) => "Z",
        (b'T', Some(_)) if frac_digits == 6 => "db",
        (b'T', Some(_)) => "+00:00",
        (b' ', None) => "display",
        (b' ', Some("+00:00")) => "display+00:00",
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    let seconds = (hour * 60 + minute) * 60 + second;
    let micros = (days * 86_400 + seconds) * 1_000_000 + frac_micros;
    Some((i, pos, Stamp { micros, format }))
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A JSON number under a key whose value is normalised: durations, pids and ports.
/// Returns the span of the number and its placeholder.
fn json_field_at(bytes: &[u8], i: usize) -> Option<Span<&'static str>> {
    // A key is `"key"`, or `\"key\"` inside a JSON document embedded as a string value.
    let key_start = if bytes[i..].starts_with(b"\\\"") {
        i + 2
    } else if bytes[i] == b'"' {
        i + 1
    } else {
        return None;
    };
    let mut j = key_start;
    while bytes.get(j).is_some_and(|b| is_word(*b)) {
        j += 1;
    }
    let closes = bytes.get(j) == Some(&b'"') || bytes[j..].starts_with(b"\\\"");
    if j == key_start || !closes {
        return None;
    }
    let key = std::str::from_utf8(&bytes[key_start..j]).ok()?;
    let close_len = if bytes.get(j) == Some(&b'"') { 1 } else { 2 };
    let placeholder = match key {
        "pid" => "<pid>",
        "port" => "<port>",
        "duration_seconds" => "<dur>",
        _ if key.ends_with("_ms") => "<dur>",
        _ => return None,
    };
    let mut k = j + close_len;
    while bytes.get(k) == Some(&b' ') {
        k += 1;
    }
    if bytes.get(k) != Some(&b':') {
        return None;
    }
    k += 1;
    while bytes.get(k) == Some(&b' ') {
        k += 1;
    }
    let start = k;
    let digits = |mut p: usize| {
        let from = p;
        while bytes.get(p).is_some_and(u8::is_ascii_digit) {
            p += 1;
        }
        (p > from).then_some(p)
    };
    if bytes.get(k) == Some(&b'-') {
        k += 1;
    }
    let mut end = digits(k)?;
    if bytes.get(end) == Some(&b'.') {
        end = digits(end + 1)?;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exp = end + 1;
        if matches!(bytes.get(exp), Some(b'+' | b'-')) {
            exp += 1;
        }
        if let Some(e) = digits(exp) {
            end = e;
        }
    }
    Some((start, end, placeholder))
}

/// The digits after `127.0.0.1:` or `localhost:`.
fn local_port_at(bytes: &[u8], i: usize) -> Option<Span<()>> {
    let prefix_len = [&b"127.0.0.1:"[..], &b"localhost:"[..]]
        .into_iter()
        .find(|p| bytes[i..].starts_with(p))?
        .len();
    let start = i + prefix_len;
    let mut end = start;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    (end > start).then_some((start, end, ()))
}

/// Instants written within one command closer than this are one instant for ranking. Python
/// stamps `created_at` and `updated_at` in separate calls, and under load the clock gives them
/// values a few milliseconds or a few hundred milliseconds apart, so without this one run can
/// rank them as two instants and the other run as one. Fixture dates are a second apart, so they
/// keep their order.
const SAME_COMMAND_MICROS: i64 = 900_000;

/// The distinct instants written in `text`, in order.
fn stamps_of(text: &str) -> BTreeSet<i64> {
    scan(text.as_bytes(), timestamp_at)
        .into_iter()
        .map(|(_, _, stamp)| stamp.micros)
        .collect()
}

/// Maps each instant of one command's output to the representative of its cluster: instants
/// less than `SAME_COMMAND_MICROS` apart, chained, form one cluster whose representative is its
/// earliest instant.
fn instant_clusters(stamps: &BTreeSet<i64>) -> BTreeMap<i64, i64> {
    let mut clusters = BTreeMap::new();
    let mut representative = None;
    let mut previous: Option<i64> = None;
    for &micros in stamps {
        let same = previous.is_some_and(|p| micros - p < SAME_COMMAND_MICROS);
        if !same {
            representative = Some(micros);
        }
        clusters.insert(micros, representative.unwrap_or(micros));
        previous = Some(micros);
    }
    clusters
}

/// First-appearance numbers for UUIDs, SHAs and worker ids, and timestamp ranks, for one run.
#[derive(Default)]
struct Numbering {
    ids: BTreeMap<String, usize>,
    shas: BTreeMap<String, usize>,
    workers: BTreeMap<String, usize>,
    /// One-based rank of each cluster representative among the run's clusters. Clusters are
    /// formed per command, see `instant_clusters`.
    ranks: BTreeMap<i64, usize>,
}

impl Numbering {
    fn from_outputs(outputs: &[&str]) -> Self {
        let mut n = Self::default();
        for text in outputs {
            for (_, _, key) in scan(text.as_bytes(), uuid_at) {
                n.id(&key);
            }
        }
        // Prefixes come after every full UUID of the run, so a prefix finds the UUID it abbreviates.
        for text in outputs {
            for (_, _, prefix) in scan(text.as_bytes(), attempt_prefix_at) {
                n.attempt(&prefix);
            }
        }
        for text in outputs {
            let bytes = text.as_bytes();
            for (_, _, key) in scan(bytes, sha_at) {
                n.sha(&key);
            }
            for (_, _, worker) in scan(bytes, worker_at) {
                n.worker(&worker);
            }
        }
        let representatives: BTreeSet<i64> = outputs
            .iter()
            .flat_map(|text| instant_clusters(&stamps_of(text)).into_values())
            .collect();
        for (rank, representative) in representatives.into_iter().enumerate() {
            n.ranks.insert(representative, rank + 1);
        }
        n
    }

    fn id(&mut self, key: &str) -> usize {
        let next = self.ids.len() + 1;
        *self.ids.entry(key.to_owned()).or_insert(next)
    }

    /// The number of the full UUID that `prefix` abbreviates, or a number of its own when the
    /// run never shows that UUID.
    fn attempt(&mut self, prefix: &str) -> usize {
        let full = self
            .ids
            .iter()
            .find(|(id, _)| id.starts_with(prefix))
            .map(|(_, n)| *n);
        full.unwrap_or_else(|| self.id(prefix))
    }

    fn sha(&mut self, key: &str) -> usize {
        let next = self.shas.len() + 1;
        *self.shas.entry(key.to_owned()).or_insert(next)
    }

    fn worker(&mut self, key: &str) -> usize {
        let next = self.workers.len() + 1;
        *self.workers.entry(key.to_owned()).or_insert(next)
    }

    /// One-based rank of an instant: the rank of its cluster in `clusters`, the map of the
    /// command the instant was written in. Every timestamp of the text being normalised is in
    /// the run's outputs, so it has a rank.
    fn rank(&self, micros: i64, clusters: &BTreeMap<i64, i64>) -> usize {
        clusters
            .get(&micros)
            .and_then(|rep| self.ranks.get(rep))
            .copied()
            .unwrap_or(1)
    }
}

/// A worker id, `<host>-<pid>-<hex6>` (Python `worker.py`: `platform.node()`, `os.getpid()`,
/// `uuid4().hex[:6]`). The host has letters and may contain hyphens; the token is bounded by
/// characters that cannot be part of one.
fn worker_at(bytes: &[u8], i: usize) -> Option<Span<String>> {
    let is_token = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_');
    if (i > 0 && is_token(bytes[i - 1])) || !is_token(*bytes.get(i)?) {
        return None;
    }
    let end = i + bytes[i..].iter().take_while(|b| is_token(**b)).count();
    let tok = &bytes[i..end];
    let n = tok.len();
    if n < 11 || tok[n - 7] != b'-' || !tok[n - 6..].iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let pid_end = n - 7;
    let pid_start = pid_end
        - tok[..pid_end]
            .iter()
            .rev()
            .take_while(|b| b.is_ascii_digit())
            .count();
    if pid_start == pid_end || pid_start == 0 || tok[pid_start - 1] != b'-' {
        return None;
    }
    let host = &tok[..pid_start - 1];
    host.iter()
        .any(u8::is_ascii_alphabetic)
        .then(|| (i, end, lower(tok)))
}

/// The list-valued id keys that the normaliser sorts (docs/rust-rewrite.md section 12, item 2).
/// Ordered lists (list outputs) are not in this set and keep their order.
const SET_LIST_KEYS: [&str; 5] = [
    "depends_on",
    "blocks",
    "child_ids",
    "project_ids",
    "repository_ids",
];

/// Lists of objects that the normaliser sorts. The `task graph` edges come from an unordered
/// dependency query, so Python's order follows the random UUIDs of the tasks (WG-3799). Graph
/// nodes are not here: they are sorted by task number, which is deterministic.
const OBJECT_LIST_KEYS: [&str; 1] = ["edges"];

/// Sorts the elements of every set-like id list and every sorted object list, after placeholder
/// substitution. Each slot keeps its own leading and trailing whitespace, so the layout of the
/// list does not change.
fn sort_set_lists(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    let candidates = SET_LIST_KEYS
        .iter()
        .map(|key| (*key, false))
        .chain(OBJECT_LIST_KEYS.iter().map(|key| (*key, true)));
    while let Some((open, objects)) = candidates
        .clone()
        .filter_map(|(key, objects)| set_list_after(text, cursor, key).map(|o| (o, objects)))
        .min_by_key(|(o, _)| *o)
    {
        let Some(close) = text[open..].find(']').map(|c| open + c) else {
            break;
        };
        out.push_str(&text[cursor..=open]);
        let content = &text[open + 1..close];
        out.push_str(&if objects {
            sort_objects(content)
        } else {
            sort_elements(content)
        });
        cursor = close;
    }
    out.push_str(&text[cursor..]);
    out
}

/// The content of a list of flat objects with its objects sorted by their text. Each object is a
/// top-level `{...}` (string-aware, so braces inside strings do not count). The text between the
/// objects, the separators and the whitespace, stays where it is.
fn sort_objects(content: &str) -> String {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let (mut depth, mut start, mut in_string, mut escaped) = (0_usize, 0_usize, false, false);
    for (i, c) in content.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    spans.push((start, i + 1));
                }
            }
            _ => {}
        }
    }
    if spans.is_empty() {
        return content.to_owned();
    }
    let mut texts: Vec<&str> = spans.iter().map(|&(s, e)| &content[s..e]).collect();
    texts.sort_unstable();
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0;
    for ((s, e), text) in spans.iter().zip(texts) {
        out.push_str(&content[cursor..*s]);
        out.push_str(text);
        cursor = *e;
    }
    out.push_str(&content[cursor..]);
    out
}

/// The position of the `[` of the first `"key": [` at or after `from`.
fn set_list_after(text: &str, from: usize, key: &str) -> Option<usize> {
    // The key as plain JSON, and escaped inside an embedded JSON string (`\"key\"`).
    [format!("\"{key}\""), format!("\\\"{key}\\\"")]
        .iter()
        .filter_map(|pattern| set_list_with(text, from, pattern))
        .min()
}

fn set_list_with(text: &str, from: usize, pattern: &str) -> Option<usize> {
    let mut at = from;
    while let Some(p) = text[at..].find(pattern).map(|p| at + p) {
        let after = &text[p + pattern.len()..];
        let list = after
            .trim_start()
            .strip_prefix(':')
            .and_then(|rest| rest.trim_start().strip_prefix('['));
        if let Some(r) = list {
            return Some(text.len() - r.len() - 1);
        }
        at = p + pattern.len();
    }
    None
}

/// The content between `[` and `]` with its elements sorted, slot by slot.
fn sort_elements(content: &str) -> String {
    if content.trim().is_empty() {
        return content.to_owned();
    }
    let slots: Vec<(&str, &str, &str)> = content
        .split(',')
        .map(|piece| {
            let elem = piece.trim();
            let lead = &piece[..piece.len() - piece.trim_start().len()];
            let trail = &piece[piece.trim_end().len()..];
            (lead, elem, trail)
        })
        .collect();
    let mut elems: Vec<&str> = slots.iter().map(|(_, e, _)| *e).collect();
    elems.sort_unstable();
    slots
        .iter()
        .zip(elems)
        .map(|((lead, _, trail), elem)| format!("{lead}{elem}{trail}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{Observed, compare, observe};
    use crate::runner::{Status, StepRecord};

    fn run(name: &str, outputs: &[(&str, &str)]) -> RunResult {
        RunResult {
            scenario: "s".into(),
            implementation: name.into(),
            roots: vec![("/tmp/abc".into(), "<tmp>")],
            steps: outputs
                .iter()
                .enumerate()
                .map(|(i, (stdout, stderr))| StepRecord {
                    index: Some(i),
                    kind: "cli",
                    label: format!("step {i}"),
                    exit: Some(0),
                    stdout: (*stdout).into(),
                    stderr: (*stderr).into(),
                    status: Status::Passed,
                    note: String::new(),
                })
                .collect(),
        }
    }

    fn norm(r: &RunResult, text: &str) -> String {
        FullNormaliser::default().normalise(r, text)
    }

    const ID1: &str = "3f2b8c1e-9a4d-4e6f-8b7a-1c2d3e4f5a6b";
    const ID2: &str = "0a1b2c3d-4e5f-4a6b-9c8d-7e6f5a4b3c2d";

    #[test]
    fn uuids_are_numbered_by_first_appearance() {
        let first = format!("{ID1} {ID2}");
        let second = format!("{ID2} {ID1} {ID1}");
        let r = run("py", &[(first.as_str(), ""), (second.as_str(), "")]);
        assert_eq!(norm(&r, &format!("{ID1}/{ID2}")), "<id:1>/<id:2>");
        assert_eq!(
            norm(&r, &format!("{ID2} {ID1} {ID1}")),
            "<id:2> <id:1> <id:1>"
        );
    }

    #[test]
    fn two_runs_with_other_uuids_compare_equal() {
        let left_text = format!("task {ID1} and {ID2}\n");
        let a = run("py", &[(left_text.as_str(), "")]);
        let other1 = "11111111-2222-4333-8444-555555555555";
        let other2 = "66666666-7777-4888-9999-aaaaaaaaaaaa";
        let right_text = format!("task {other1} and {other2}\n");
        let b = run("rs", &[(right_text.as_str(), "")]);
        let n = FullNormaliser::default();
        let left = observe(&a, &n);
        let right = observe(&b, &n);
        assert!(compare("s", &left, &right, &[]).is_empty());
        assert_eq!(left[0].stdout, "task <id:1> and <id:2>\n");
    }

    #[test]
    fn short_or_embedded_hex_is_not_a_uuid() {
        let r = run("py", &[("", "")]);
        let text = "abc12345-9a4d-4e6f-8b7a-1c2d3e4f5a6b0 deadbeef";
        assert_eq!(norm(&r, text), text);
    }

    #[test]
    fn shas_are_numbered_and_short_hex_is_left_alone() {
        let sha1 = "a".repeat(40);
        let sha2 = "1234567890abcdef1234567890abcdef12345678";
        let shas = format!("{sha1} {sha2}");
        let r = run("py", &[(shas.as_str(), "")]);
        assert_eq!(
            norm(&r, &format!("{sha2} {sha1} abc1234")),
            "<sha:2> <sha:1> abc1234"
        );
    }

    #[test]
    fn timestamps_keep_format_and_rank_by_instant() {
        let text = "2026-10-25T10:00:00Z 2026-10-25T09:00:00+00:00 2026-10-25T10:00:00.000000+00:00 2026-10-25 09:00:00";
        let r = run("py", &[(text, "")]);
        assert_eq!(
            norm(&r, text),
            "<ts:2|Z> <ts:1|+00:00> <ts:2|db> <ts:1|display>"
        );
    }

    #[test]
    fn timestamps_of_two_runs_compare_equal_by_ordering() {
        let a = run(
            "py",
            &[(
                "created 2026-01-02T03:04:05Z then 2026-01-02T03:05:00Z\n",
                "",
            )],
        );
        let b = run(
            "rs",
            &[(
                "created 2031-07-08T00:00:01Z then 2031-07-08T00:09:59Z\n",
                "",
            )],
        );
        let n = FullNormaliser::default();
        assert!(compare("s", &observe(&a, &n), &observe(&b, &n), &[]).is_empty());
        // A different ordering is still a difference.
        let c = run(
            "rs",
            &[(
                "created 2031-07-08T00:09:59Z then 2031-07-08T00:00:01Z\n",
                "",
            )],
        );
        assert!(!compare("s", &observe(&a, &n), &observe(&c, &n), &[]).is_empty());
    }

    #[test]
    fn stamps_a_clock_tick_apart_are_one_instant() {
        // `created_at` and `updated_at` written in separate calls: equal in one run, a few
        // milliseconds apart in the other. Both runs must normalise to the same text.
        let n = FullNormaliser::default();
        let same = "created 2026-10-26T06:34:41.462098Z updated 2026-10-26T06:34:41.462098Z";
        let apart = "created 2026-10-26T06:34:41.462098Z updated 2026-10-26T06:34:41.466000Z";
        let a = run("py", &[(same, "")]);
        let b = run("rs", &[(apart, "")]);
        assert!(compare("s", &observe(&a, &n), &observe(&b, &n), &[]).is_empty());
        // A second apart is a different instant, and the order is kept across the whole run.
        let later = "created 2026-10-26T06:34:41.462098Z updated 2026-10-26T06:34:42.462098Z";
        let c = run("rs", &[(later, "")]);
        assert!(!compare("s", &observe(&a, &n), &observe(&c, &n), &[]).is_empty());
    }

    #[test]
    fn json_durations_pids_and_ports_are_placeholders() {
        let r = run("py", &[("", "")]);
        let text = r#"{"duration_ms": 12.5, "queue_wait_ms": null, "duration_seconds": 3, "pid": 4242, "port": 8765, "count": 7}"#;
        assert_eq!(
            norm(&r, text),
            r#"{"duration_ms": <dur>, "queue_wait_ms": null, "duration_seconds": <dur>, "pid": <pid>, "port": <port>, "count": 7}"#
        );
    }

    #[test]
    fn local_ports_and_literals_and_roots_are_replaced() {
        let mut n = FullNormaliser::default();
        n.literals.push(("s3cr3t-token".into(), "<token>".into()));
        let r = run("py", &[("", "")]);
        let out = n.normalise(
            &r,
            "http://127.0.0.1:51234/api token s3cr3t-token at /tmp/abc/x",
        );
        assert_eq!(out, "http://127.0.0.1:<port>/api token <token> at <tmp>/x");
    }

    #[test]
    fn attempt_prefixes_take_the_placeholder_of_their_full_uuid() {
        let attempt = "wg/WG-1/attempt-1-3f2b8c1e";
        let full_seen_later = format!("{attempt} {ID1}");
        let r = run("py", &[(full_seen_later.as_str(), "")]);
        let n = FullNormaliser::default();
        assert_eq!(
            n.normalise(&r, &full_seen_later),
            "wg/WG-1/attempt-1-<id:1> <id:1>"
        );
        // The same prefix in another run, with another UUID, still matches its UUID.
        let other = "11111111-2222-4333-8444-555555555555";
        let text = format!("wg/WG-1/attempt-1-{} {other}", &other[..8]);
        let rs = run("rs", &[(text.as_str(), "")]);
        assert_eq!(n.normalise(&rs, &text), "wg/WG-1/attempt-1-<id:1> <id:1>");
        // A prefix that no UUID of the run shows gets a number of its own.
        assert_eq!(
            n.normalise(&r, "wg/WG-2/attempt-2-abcdef12"),
            "wg/WG-2/attempt-2-<id:2>"
        );
        // Words that merely contain `attempt-` and a short hex run are left alone.
        assert_eq!(
            n.normalise(&r, "myattempt-1-3f2b8c1e x attempt-x-3f2b8c1e"),
            "myattempt-1-3f2b8c1e x attempt-x-3f2b8c1e"
        );
    }

    #[test]
    fn stamps_within_one_command_rank_together_across_commands_they_do_not() {
        let n = FullNormaliser::default();
        // 850 ms apart in one command: one instant.
        let one = "a 2026-10-26T06:34:41.000000Z b 2026-10-26T06:34:41.850000Z\n";
        let other = "a 2026-10-26T06:34:41.000000Z b 2026-10-26T06:34:41.000000Z\n";
        let a = run("py", &[(one, "")]);
        let b = run("rs", &[(other, "")]);
        assert!(compare("s", &observe(&a, &n), &observe(&b, &n), &[]).is_empty());
        // The same two instants written by two commands stay two instants.
        let first = "a 2026-10-26T06:34:41.000000Z\n";
        let second = "b 2026-10-26T06:34:41.850000Z\n";
        let c = run("py", &[(first, ""), (second, "")]);
        let d = run("rs", &[(first, ""), (first, "")]);
        assert!(!compare("s", &observe(&c, &n), &observe(&d, &n), &[]).is_empty());
    }

    #[test]
    fn fixture_dates_a_second_apart_keep_their_order() {
        let n = FullNormaliser::default();
        let text = "2026-01-01T00:00:00Z 2026-01-01T00:00:01Z";
        let r = run("py", &[(text, "")]);
        assert_eq!(n.normalise(&r, text), "<ts:1|Z> <ts:2|Z>");
    }

    #[test]
    fn expires_at_form_is_a_timestamp() {
        let n = FullNormaliser::default();
        let text = "expires 2026-10-26 06:34:41.462098+00:00 and 2026-10-26 06:34:41.462098+00:00";
        let r = run("py", &[(text, "")]);
        assert_eq!(
            n.normalise(&r, text),
            "expires <ts:1|display+00:00> and <ts:1|display+00:00>"
        );
    }

    #[test]
    fn worker_ids_become_numbered_placeholders() {
        let n = FullNormaliser::default();
        let text = "worker DESKTOP-1IULO99-9252-1209b5 started\n\"DESKTOP-1IULO99-27040-1b35e0\"\nother-host-7-abcdef and word-12-zz";
        let r = run("py", &[(text, "")]);
        assert_eq!(
            n.normalise(&r, text),
            "worker <worker:1> started\n\"<worker:2>\"\n<worker:3> and word-12-zz"
        );
        // Another run, other pids and suffixes, same placeholders.
        let other = "worker DESKTOP-1IULO99-111-aaaaaa started\n\"DESKTOP-1IULO99-222-bbbbbb\"\nother-host-9-cccccc and word-12-zz";
        let s = run("rs", &[(other, "")]);
        assert_eq!(n.normalise(&s, other), n.normalise(&r, text));
    }

    #[test]
    fn crlf_and_lf_compare_equal() {
        let n = FullNormaliser::default();
        let a = run("py", &[("line one\r\nline two\r\n", "")]);
        let b = run("rs", &[("line one\nline two\n", "")]);
        assert!(compare("s", &observe(&a, &n), &observe(&b, &n), &[]).is_empty());
    }

    #[test]
    fn set_like_id_lists_are_sorted_and_ordered_lists_are_not() {
        let n = FullNormaliser::default();
        let r = run("py", &[("", "")]);
        let text = r#"{"depends_on": ["WG-5", "WG-4"], "blocks": [], "project_ids": ["<id:2>", "<id:1>"], "edges": [["WG-5", "WG-4"]]}"#;
        assert_eq!(
            n.normalise(&r, text),
            r#"{"depends_on": ["WG-4", "WG-5"], "blocks": [], "project_ids": ["<id:1>", "<id:2>"], "edges": [["WG-5", "WG-4"]]}"#
        );
        // Pretty-printed lists keep their layout.
        let pretty = "\"repository_ids\": [\n    \"b\",\n    \"a\"\n  ]";
        assert_eq!(
            n.normalise(&r, pretty),
            "\"repository_ids\": [\n    \"a\",\n    \"b\"\n  ]"
        );
    }

    #[test]
    fn escaped_json_inside_a_string_is_normalised_like_plain_json() {
        let n = FullNormaliser::default();
        let r = run("py", &[("", "")]);
        let text = format!(
            r#"{{"health_detail": "{{\"id\": \"{ID1}\", \"duration_ms\": 12, \"pid\": 4242, \"depends_on\": [\"<id:2>\", \"<id:1>\"]}}"}}"#
        );
        assert_eq!(
            n.normalise(&r, &text),
            r#"{"health_detail": "{\"id\": \"<id:1>\", \"duration_ms\": <dur>, \"pid\": <pid>, \"depends_on\": [\"<id:1>\", \"<id:2>\"]}"}"#
        );
    }

    #[test]
    fn graph_edges_are_sorted_and_nodes_keep_their_order() {
        let n = FullNormaliser::default();
        let r = run("py", &[("", "")]);
        // Edges in the order the UUID-keyed query returned them: the layout stays, the objects move.
        let edges = "{\n  \"nodes\": [\n    {\"id\": \"b\", \"tags\": [\"x\", \"y\"]},\n    {\"id\": \"a\", \"tags\": []}\n  ],\n  \"edges\": [\n    {\n      \"source\": \"<id:2>\",\n      \"target\": \"<id:3>\",\n      \"kind\": \"depends_on\"\n    },\n    {\n      \"source\": \"<id:1>\",\n      \"target\": \"<id:3>\",\n      \"kind\": \"parent\"\n    }\n  ]\n}";
        let sorted = "{\n  \"nodes\": [\n    {\"id\": \"b\", \"tags\": [\"x\", \"y\"]},\n    {\"id\": \"a\", \"tags\": []}\n  ],\n  \"edges\": [\n    {\n      \"source\": \"<id:1>\",\n      \"target\": \"<id:3>\",\n      \"kind\": \"parent\"\n    },\n    {\n      \"source\": \"<id:2>\",\n      \"target\": \"<id:3>\",\n      \"kind\": \"depends_on\"\n    }\n  ]\n}";
        assert_eq!(n.normalise(&r, edges), sorted);
        // Sorting is stable across runs: the other order normalises to the same text.
        let reversed = "\"edges\": [{\"source\": \"<id:1>\", \"kind\": \"parent\"}, {\"source\": \"<id:2>\", \"kind\": \"depends_on\"}]";
        let back = "\"edges\": [{\"source\": \"<id:2>\", \"kind\": \"depends_on\"}, {\"source\": \"<id:1>\", \"kind\": \"parent\"}]";
        assert_eq!(n.normalise(&r, reversed), n.normalise(&r, back));
    }

    #[test]
    fn observed_structure_is_kept() {
        let r = run("py", &[("{\"id\": \"WG-1\"}\n", "")]);
        let obs: Vec<Observed> = observe(&r, &FullNormaliser::default());
        assert_eq!(obs[0].stdout, "{\"id\": \"WG-1\"}\n");
    }
}
