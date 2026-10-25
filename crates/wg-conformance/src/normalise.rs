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
        let mut out = RootsNormaliser.normalise(run, text);
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

        let mut spans: Vec<(usize, usize, String)> = Vec::new();
        for (s, e, key) in scan(bytes, uuid_at) {
            spans.push((s, e, format!("<id:{}>", numbering.id(&key))));
        }
        for (s, e, key) in scan(bytes, sha_at) {
            spans.push((s, e, format!("<sha:{}>", numbering.sha(&key))));
        }
        for (s, e, stamp) in scan(bytes, timestamp_at) {
            let rank = numbering.rank(stamp.micros);
            spans.push((s, e, format!("<ts:{rank}|{}>", stamp.format)));
        }
        for (s, e, placeholder) in scan(bytes, json_field_at) {
            spans.push((s, e, placeholder.to_owned()));
        }
        for (s, e, ()) in scan(bytes, local_port_at) {
            spans.push((s, e, "<port>".to_owned()));
        }
        splice(&out, spans)
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
    if bytes[i] != b'"' {
        return None;
    }
    let mut j = i + 1;
    while bytes.get(j).is_some_and(|b| is_word(*b)) {
        j += 1;
    }
    if j == i + 1 || bytes.get(j) != Some(&b'"') {
        return None;
    }
    let key = std::str::from_utf8(&bytes[i + 1..j]).ok()?;
    let placeholder = match key {
        "pid" => "<pid>",
        "port" => "<port>",
        "duration_seconds" => "<dur>",
        _ if key.ends_with("_ms") => "<dur>",
        _ => return None,
    };
    let mut k = j + 1;
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

/// First-appearance numbers for UUIDs and SHAs, and timestamp ranks, for one run.
#[derive(Default)]
struct Numbering {
    ids: BTreeMap<String, usize>,
    shas: BTreeMap<String, usize>,
    stamps: BTreeSet<i64>,
}

impl Numbering {
    fn from_outputs(outputs: &[&str]) -> Self {
        let mut n = Self::default();
        for text in outputs {
            let bytes = text.as_bytes();
            for (_, _, key) in scan(bytes, uuid_at) {
                n.id(&key);
            }
            for (_, _, key) in scan(bytes, sha_at) {
                n.sha(&key);
            }
            for (_, _, stamp) in scan(bytes, timestamp_at) {
                n.stamps.insert(stamp.micros);
            }
        }
        n
    }

    fn id(&mut self, key: &str) -> usize {
        let next = self.ids.len() + 1;
        *self.ids.entry(key.to_owned()).or_insert(next)
    }

    fn sha(&mut self, key: &str) -> usize {
        let next = self.shas.len() + 1;
        *self.shas.entry(key.to_owned()).or_insert(next)
    }

    /// One-based rank of an instant among the distinct instants of the run.
    fn rank(&mut self, micros: i64) -> usize {
        self.stamps.insert(micros);
        self.stamps.range(..micros).count() + 1
    }
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
    fn observed_structure_is_kept() {
        let r = run("py", &[("{\"id\": \"WG-1\"}\n", "")]);
        let obs: Vec<Observed> = observe(&r, &FullNormaliser::default());
        assert_eq!(obs[0].stdout, "{\"id\": \"WG-1\"}\n");
    }
}
