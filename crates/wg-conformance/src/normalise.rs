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

/// How far outside a step's recorded wall-clock window a timestamp may fall and still belong to
/// it: clock granularity (Windows ticks at about 15 ms) and the gap between a process stamping a
/// row and the runner noticing it returned.
const WINDOW_MARGIN_MICROS: i64 = 25_000;

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
        let windows = step_windows(run);
        let mut numbering = Numbering::from_outputs(&outputs, &windows);
        let bytes = out.as_bytes();
        // The archive size of a backup is a zip byte count that differs between two Python runs,
        // so it is masked only in output that names a backup format. Other `bytes` keys stay.
        let archive = out.contains("\"format\": \"workgraph-backup-");

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
        for (s, e, stamp) in scan(bytes, any_timestamp_at) {
            let placeholder = match step_of(&windows, &stamp) {
                Some(k) => format!("<ts@{k}|{}>", stamp.format),
                None => format!("<ts:{}|{}>", numbering.rank(stamp.micros), stamp.format),
            };
            spans.push((s, e, placeholder));
        }
        for (s, e, worker) in scan(bytes, worker_at) {
            spans.push((s, e, format!("<worker:{}>", numbering.worker(&worker))));
        }
        for (s, e, placeholder) in scan(bytes, json_field_at) {
            spans.push((s, e, placeholder.to_owned()));
        }
        for (s, e, masked) in scan(bytes, ms_object_at) {
            spans.push((s, e, masked));
        }
        if archive {
            for (s, e, placeholder) in scan(bytes, archive_bytes_at) {
                spans.push((s, e, placeholder.to_owned()));
            }
        }
        for (s, e, ()) in scan(bytes, local_port_at) {
            spans.push((s, e, "<port>".to_owned()));
        }
        // Bare calendar dates near the run day. Added last, so a timestamp span found above keeps
        // its overlap: `splice` drops the later of two overlapping spans.
        if let Some(today) = run.started.map(|micros| micros.div_euclid(MICROS_PER_DAY)) {
            for (s, e, days) in scan(bytes, date_at) {
                if let Some(placeholder) = today_placeholder(days, today) {
                    spans.push((s, e, placeholder));
                }
            }
        }
        sort_set_lists(&splice(&out, spans))
    }
}

const MICROS_PER_DAY: i64 = 86_400_000_000;

/// How far from the run day a bare date may fall and still be rewritten to a `<today...>`
/// placeholder. Beyond it the date is kept, so a scenario's fixture dates stay as written.
const TODAY_WINDOW_DAYS: i64 = 400;

/// A bare calendar date `YYYY-MM-DD` and the days since 1970-01-01 it names. Neither neighbour may
/// be a word character, so a date glued to more text is not one: a timestamp's `T` (a naive
/// `2026-10-30T22:16:45` is left as written), or a digit run.
fn date_at(bytes: &[u8], i: usize) -> Option<Span<i64>> {
    let end = i + 10;
    if end > bytes.len() || !bounded(bytes, i, end) || bytes[i + 4] != b'-' || bytes[i + 7] != b'-'
    {
        return None;
    }
    let digits = |from: usize, n: usize| -> Option<i64> {
        let part = bytes.get(from..from + n)?;
        part.iter().all(u8::is_ascii_digit).then(|| {
            part.iter()
                .fold(0_i64, |acc, d| acc * 10 + i64::from(d - b'0'))
        })
    };
    let year = digits(i, 4)?;
    let month = digits(i + 5, 2)?;
    let day = digits(i + 8, 2)?;
    ((1..=12).contains(&month) && (1..=31).contains(&day))
        .then(|| (i, end, days_from_civil(year, month, day)))
}

/// The `<today...>` placeholder of a date `days` since the epoch on a run whose day is `today`:
/// `<today>`, `<today-N>` for N days before, `<today+N>` for N days after. `None` outside
/// `TODAY_WINDOW_DAYS` either side.
fn today_placeholder(days: i64, today: i64) -> Option<String> {
    let delta = days - today;
    if delta.abs() > TODAY_WINDOW_DAYS {
        return None;
    }
    Some(match delta {
        0 => "<today>".to_owned(),
        d if d < 0 => format!("<today-{}>", -d),
        d => format!("<today+{d}>"),
    })
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
    /// Width of the instant the text can denote: a second for forms without a fraction (the
    /// writer truncated), one microsecond otherwise.
    span: i64,
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
    let span = if frac_digits == 0 { 1_000_000 } else { 1 };
    Some((
        i,
        pos,
        Stamp {
            micros,
            span,
            format,
        },
    ))
}

/// Any timestamp this normaliser knows: the extended forms of `timestamp_at`, or the compact form
/// `YYYYMMDDTHHMMSSZ` that backup archives use (`20261026T144947Z`).
fn any_timestamp_at(bytes: &[u8], i: usize) -> Option<Span<Stamp>> {
    timestamp_at(bytes, i).or_else(|| compact_stamp_at(bytes, i))
}

/// `YYYYMMDDTHHMMSSZ`, exactly sixteen characters with no word character on either side.
fn compact_stamp_at(bytes: &[u8], i: usize) -> Option<Span<Stamp>> {
    const LEN: usize = 16;
    let end = i + LEN;
    if end > bytes.len() || !bounded(bytes, i, end) {
        return None;
    }
    let digits = |from: usize, n: usize| -> Option<i64> {
        let part = bytes.get(from..from + n)?;
        part.iter().all(u8::is_ascii_digit).then(|| {
            part.iter()
                .fold(0_i64, |acc, d| acc * 10 + i64::from(d - b'0'))
        })
    };
    let shaped = bytes[i + 8] == b'T' && bytes[end - 1] == b'Z';
    if !shaped {
        return None;
    }
    let year = digits(i, 4)?;
    let month = digits(i + 4, 2)?;
    let day = digits(i + 6, 2)?;
    let hour = digits(i + 9, 2)?;
    let minute = digits(i + 11, 2)?;
    let second = digits(i + 13, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = (hour * 60 + minute) * 60 + second;
    let micros = (days * 86_400 + seconds) * 1_000_000;
    Some((
        i,
        end,
        Stamp {
            micros,
            span: 1_000_000,
            format: "compact",
        },
    ))
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
    keyed_number_at(bytes, i, |key| match key {
        "pid" => Some("<pid>"),
        "port" => Some("<port>"),
        "duration_seconds" | "seconds" => Some("<dur>"),
        _ if is_ms_key(key) => Some("<dur>"),
        _ => None,
    })
}

/// The `bytes` number of a backup archive (`"bytes": 27069`). Only the caller that has seen the
/// backup format masks it.
fn archive_bytes_at(bytes: &[u8], i: usize) -> Option<Span<&'static str>> {
    keyed_number_at(bytes, i, |key| (key == "bytes").then_some("<bytes>"))
}

/// A JSON key that names a duration in milliseconds: `*_ms`, or a key with `_ms_` inside such as
/// `median_elapsed_ms_cross_repo`.
fn is_ms_key(key: &str) -> bool {
    key.ends_with("_ms") || key.contains("_ms_")
}

/// The key at `i` and the position of its value. A key is `"key"`, or `\"key\"` inside a JSON
/// document embedded as a string value. The value starts after the colon and the spaces around it.
fn key_value_at(bytes: &[u8], i: usize) -> Option<(&str, usize)> {
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
    Some((key, k))
}

/// A JSON number under a key that `placeholder` names, with the key's placeholder. The span
/// covers the number only.
fn keyed_number_at(
    bytes: &[u8],
    i: usize,
    placeholder: impl Fn(&str) -> Option<&'static str>,
) -> Option<Span<&'static str>> {
    let (key, start) = key_value_at(bytes, i)?;
    let placeholder = placeholder(key)?;
    let mut k = start;
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

/// A `*_ms` key whose value is a flat object of figures, such as the `elapsed_ms` distribution
/// (`{"n": 4, "p25": 96323.75, ...}`). Every number in it is a duration except the sample count
/// `n`, which is exact and stays. Returns the span of the object and its text with the durations
/// replaced.
fn ms_object_at(bytes: &[u8], i: usize) -> Option<Span<String>> {
    let (key, start) = key_value_at(bytes, i)?;
    if !is_ms_key(key) || bytes.get(start) != Some(&b'{') {
        return None;
    }
    let end = start + bytes[start..].iter().position(|b| *b == b'}')? + 1;
    let object = std::str::from_utf8(&bytes[start..end]).ok()?;
    let durations: Vec<Span<String>> = scan(object.as_bytes(), |b, j| {
        keyed_number_at(b, j, |k: &str| (k != "n").then_some("<dur>"))
    })
    .into_iter()
    .map(|(s, e, p)| (s, e, p.to_owned()))
    .collect();
    Some((start, end, splice(object, durations)))
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

/// A step's wall-clock window in microseconds since the epoch, with the step's position in the
/// run's step list (setup steps first), which names it in the placeholder.
type Window = (usize, i64, i64);

/// The windows of the run's steps that recorded one.
fn step_windows(run: &RunResult) -> Vec<Window> {
    run.steps
        .iter()
        .enumerate()
        .filter_map(|(k, s)| s.window.map(|(a, b)| (k, a, b)))
        .collect()
}

/// The step that wrote `stamp`: the one whose window holds it, else the nearest window within
/// `WINDOW_MARGIN_MICROS` (earlier step on a tie). Provenance replaces proximity clustering, so
/// the answer does not depend on how fast the run was: everything a step writes is one instant,
/// however far from the next step's writes. `None` for dates the scenario supplied (fixtures,
/// commit dates), which keep their rank.
fn step_of(windows: &[Window], stamp: &Stamp) -> Option<usize> {
    let (lo, hi) = (stamp.micros, stamp.micros + stamp.span);
    windows
        .iter()
        .filter_map(|&(k, start, end)| {
            let gap = (start - hi).max(lo - end).max(0);
            (gap <= WINDOW_MARGIN_MICROS).then_some((gap, k))
        })
        .min()
        .map(|(_, k)| k)
}

/// First-appearance numbers for UUIDs, SHAs and worker ids, and timestamp ranks, for one run.
#[derive(Default)]
struct Numbering {
    ids: BTreeMap<String, usize>,
    shas: BTreeMap<String, usize>,
    workers: BTreeMap<String, usize>,
    /// One-based rank of each instant no step window holds, among the run's such instants.
    ranks: BTreeMap<i64, usize>,
}

impl Numbering {
    fn from_outputs(outputs: &[&str], windows: &[Window]) -> Self {
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
        let unattributed: BTreeSet<i64> = outputs
            .iter()
            .flat_map(|text| scan(text.as_bytes(), any_timestamp_at))
            .map(|(_, _, stamp)| stamp)
            .filter(|stamp| step_of(windows, stamp).is_none())
            .map(|stamp| stamp.micros)
            .collect();
        for (rank, micros) in unattributed.into_iter().enumerate() {
            n.ranks.insert(micros, rank + 1);
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

    /// One-based rank of an instant no step window holds. Every timestamp of the text being
    /// normalised is in the run's outputs, so it has a rank.
    fn rank(&self, micros: i64) -> usize {
        self.ranks.get(&micros).copied().unwrap_or(1)
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
const SET_LIST_KEYS: [&str; 6] = [
    "depends_on",
    "blocks",
    "child_ids",
    "project_ids",
    "repository_ids",
    "pair",
];

/// Lists of objects that the normaliser sorts. The `task graph` edges come from an unordered
/// dependency query, so Python's order follows the random UUIDs of the tasks (WG-3799). Graph
/// nodes are not here: they are sorted by task number, which is deterministic. Coupling evidence
/// (one object per task) comes out of an unordered query the same way (WG-3462).
const OBJECT_LIST_KEYS: [&str; 2] = ["edges", "evidence"];

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
                    window: None,
                })
                .collect(),
            started: None,
        }
    }

    /// A run whose steps carry wall-clock windows `(start, end)` in microseconds.
    fn run_w(name: &str, steps: &[(&str, (i64, i64))]) -> RunResult {
        let mut r = run(
            name,
            &steps.iter().map(|(t, _)| (*t, "")).collect::<Vec<_>>(),
        );
        for (rec, (_, w)) in r.steps.iter_mut().zip(steps) {
            rec.window = Some(*w);
        }
        r
    }

    /// Microseconds since the epoch of noon UTC on the given date: a run that started that day.
    fn run_day(year: i64, month: i64, day: i64) -> i64 {
        (days_from_civil(year, month, day) * 86_400 + 12 * 3600) * 1_000_000
    }

    /// Microseconds since the epoch of `2026-10-26T06:34:<sec>.<ms>Z`.
    fn at(sec: i64, ms: i64) -> i64 {
        (days_from_civil(2026, 10, 26) * 86_400 + 6 * 3600 + 34 * 60 + sec) * 1_000_000 + ms * 1000
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
    fn a_steps_created_and_updated_collapse_to_its_step() {
        let n = FullNormaliser::default();
        let text = "created 2026-10-26T06:34:41.462098Z updated 2026-10-26T06:34:41.466000Z";
        let r = run_w(
            "py",
            &[
                ("", (at(40, 0), at(40, 100))),
                (text, (at(41, 400), at(41, 500))),
            ],
        );
        assert_eq!(n.normalise(&r, text), "created <ts@1|Z> updated <ts@1|Z>");
    }

    /// Two steps, the second echoing both steps' stamps, with the steps `gap_ms` apart.
    fn two_steps(gap_ms: i64) -> RunResult {
        let (s0, e0) = (at(10, 0), at(10, 20));
        let (s1, e1) = (e0 + gap_ms * 1000, e0 + gap_ms * 1000 + 20_000);
        let first = "made 2026-10-26T06:34:10.005000Z".to_owned();
        let stamp = |micros: i64| {
            let ms = (micros - at(0, 0)) / 1000;
            format!("2026-10-26T06:34:{:02}.{:03}000Z", ms / 1000, ms % 1000)
        };
        let second = format!("saw {} and {}", stamp(s0 + 5000), stamp(s1 + 5000));
        run_w(
            "py",
            &[(first.as_str(), (s0, e0)), (second.as_str(), (s1, e1))],
        )
    }

    #[test]
    fn same_scenario_normalises_identically_whatever_the_speed() {
        let n = FullNormaliser::default();
        let (fast, slow) = (two_steps(50), two_steps(3000));
        let norm_all = |r: &RunResult| -> Vec<String> {
            r.steps.iter().map(|s| n.normalise(r, &s.stdout)).collect()
        };
        assert_eq!(norm_all(&fast), norm_all(&slow));
        assert_eq!(norm_all(&fast)[1], "saw <ts@0|Z> and <ts@1|Z>");
    }

    #[test]
    fn dates_outside_every_window_keep_their_rank() {
        let n = FullNormaliser::default();
        let text =
            "due 2026-01-01T00:00:00Z at 2026-10-26T06:34:41.100000Z then 2026-01-01T00:00:01Z";
        let r = run_w("py", &[(text, (at(41, 0), at(41, 200)))]);
        assert_eq!(
            n.normalise(&r, text),
            "due <ts:1|Z> at <ts@0|Z> then <ts:2|Z>"
        );
    }

    #[test]
    fn whole_second_stamps_belong_to_the_step_they_were_truncated_in() {
        let n = FullNormaliser::default();
        let text = "2026-10-26 06:34:41 and 20261026T063441Z";
        let r = run_w("py", &[(text, (at(41, 600), at(41, 700)))]);
        assert_eq!(n.normalise(&r, text), "<ts@0|display> and <ts@0|compact>");
    }

    #[test]
    fn overlapping_margins_prefer_the_window_that_holds_the_stamp() {
        let n = FullNormaliser::default();
        let text = "2026-10-26T06:34:41.010000Z 2026-10-26T06:34:41.035000Z";
        let r = run_w(
            "py",
            &[
                (text, (at(41, 0), at(41, 20))),
                ("", (at(41, 30), at(41, 50))),
            ],
        );
        assert_eq!(n.normalise(&r, text), "<ts@0|Z> <ts@1|Z>");
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

    #[test]
    fn compact_backup_stamps_rank_by_instant() {
        let n = FullNormaliser::default();
        let text = "created 20261026T144947Z then 20261026T145157Z again 20261026T144947Z";
        let r = run("py", &[(text, "")]);
        assert_eq!(
            n.normalise(&r, text),
            "created <ts:1|compact> then <ts:2|compact> again <ts:1|compact>"
        );
        // Another run one second apart on the same ordering compares equal, a different order does not.
        let same = "created 20270101T000001Z then 20270101T000009Z again 20270101T000001Z";
        let rs = run("rs", &[(same, "")]);
        assert!(compare("s", &observe(&r, &n), &observe(&rs, &n), &[]).is_empty());
        // Digit runs that are not a compact stamp are left alone.
        let other = "id 1234567T123456Z 20261026T1449Z x20261026T144947Z";
        assert_eq!(n.normalise(&r, other), other);
    }

    #[test]
    fn archive_byte_counts_are_masked_only_in_backup_output() {
        let n = FullNormaliser::default();
        let backup = "{\n  \"path\": \"<tmp>/b.zip\",\n  \"bytes\": 27069,\n  \"format\": \"workgraph-backup-v1\"\n}";
        let r = run("py", &[(backup, "")]);
        assert_eq!(
            n.normalise(&r, backup),
            "{\n  \"path\": \"<tmp>/b.zip\",\n  \"bytes\": <bytes>,\n  \"format\": \"workgraph-backup-v1\"\n}"
        );
        let other = "{\"bytes\": 775, \"format\": \"csv\"}";
        assert_eq!(n.normalise(&r, other), other);
    }

    #[test]
    fn sandbox_paths_named_by_step_env_take_their_root_placeholder() {
        // A step's env value is resolved against the sandbox before the step runs (`{home}`,
        // `{repos}/second`), so the path it leaves in output is a sandbox path. The most specific
        // root names it: a second home under the repos dir is `<tmp>/repos/...`, not `<home>`.
        let n = FullNormaliser::default();
        let mut r = run("rs", &[("", "")]);
        r.roots = vec![
            ("/w/sb/repos".into(), "<tmp>/repos"),
            ("/w/sb/home".into(), "<home>"),
            ("/w/sb".into(), "<tmp>"),
        ];
        let text = "home /w/sb/home/.wg second /w/sb/repos/second/.wg repo /w/sb/repos/a";
        assert_eq!(
            n.normalise(&r, text),
            "home <home>/.wg second <tmp>/repos/second/.wg repo <tmp>/repos/a"
        );
    }

    #[test]
    fn ms_keys_with_object_values_and_mid_name_ms_are_durations() {
        let n = FullNormaliser::default();
        let r = run("py", &[("", "")]);
        let text = "{\"median_elapsed_ms_cross_repo\": 110181.5, \"median_elapsed_ms_single_repo\": null, \"distributions\": {\"elapsed_ms\": {\"n\": 4, \"p25\": 96323.75, \"median\": null, \"max\": 132215}}, \"msg_count\": 3}";
        assert_eq!(
            n.normalise(&r, text),
            "{\"median_elapsed_ms_cross_repo\": <dur>, \"median_elapsed_ms_single_repo\": null, \"distributions\": {\"elapsed_ms\": {\"n\": 4, \"p25\": <dur>, \"median\": null, \"max\": <dur>}}, \"msg_count\": 3}"
        );
    }

    #[test]
    fn coupling_evidence_and_pairs_are_set_like() {
        let n = FullNormaliser::default();
        let r = run("py", &[("", "")]);
        let text = r#"{"pair": ["shared-sdk", "auth-service"], "evidence": [{"task_key": "WG-4"}, {"task_key": "WG-2"}], "other": ["b", "a"]}"#;
        assert_eq!(
            n.normalise(&r, text),
            r#"{"pair": ["auth-service", "shared-sdk"], "evidence": [{"task_key": "WG-2"}, {"task_key": "WG-4"}], "other": ["b", "a"]}"#
        );
    }

    #[test]
    fn bare_dates_near_the_run_day_are_today_placeholders() {
        let n = FullNormaliser::default();
        let mut r = run("py", &[("", "")]);
        r.started = Some(run_day(2026, 10, 31));
        let text = "Retrospective 2026-10-17 - 2026-10-31, period 2026-10-31, next 2026-11-05, \
                    old 2025-03-26, far 2027-12-31, stamp 2026-10-30T22:16:45Z, naive 2026-10-30T22:16:45";
        assert_eq!(
            n.normalise(&r, text),
            "Retrospective <today-14> - <today>, period <today>, next <today+5>, \
             old 2025-03-26, far 2027-12-31, stamp <ts:1|Z>, naive 2026-10-30T22:16:45"
        );
        // The same golden text on another day compares equal once both sides are normalised.
        let mut later = run("rs", &[("", "")]);
        later.started = Some(run_day(2026, 11, 1));
        let shifted = "Retrospective 2026-10-18 - 2026-11-01, period 2026-11-01, next 2026-11-06, \
                       old 2025-03-26, far 2027-12-31, stamp 2026-10-31T22:16:45Z, naive 2026-10-30T22:16:45";
        assert_eq!(
            n.normalise(&later, shifted),
            "Retrospective <today-14> - <today>, period <today>, next <today+5>, \
             old 2025-03-26, far 2027-12-31, stamp <ts:1|Z>, naive 2026-10-30T22:16:45"
        );
    }

    #[test]
    fn today_window_is_400_days_either_side() {
        let today = days_from_civil(2026, 10, 31);
        assert_eq!(today_placeholder(today, today).as_deref(), Some("<today>"));
        assert_eq!(
            today_placeholder(today - 400, today).as_deref(),
            Some("<today-400>")
        );
        assert_eq!(
            today_placeholder(today + 400, today).as_deref(),
            Some("<today+400>")
        );
        assert_eq!(today_placeholder(today - 401, today), None);
        assert_eq!(today_placeholder(today + 401, today), None);
    }

    #[test]
    fn dates_are_left_alone_without_a_run_clock_or_inside_other_text() {
        let n = FullNormaliser::default();
        let text = "2026-10-30 x20261030 2026-10-301 2026-10-30_v2";
        let no_clock = run("py", &[("", "")]);
        assert_eq!(n.normalise(&no_clock, text), text);
        let mut r = run("py", &[("", "")]);
        r.started = Some(run_day(2026, 10, 31));
        assert_eq!(
            n.normalise(&r, text),
            "<today-1> x20261030 2026-10-301 2026-10-30_v2"
        );
    }
}
