//! Comparing runs, the divergence allowlist, goldens and the per-file report.

use crate::runner::{RunResult, Status};
use serde_json::{Value, json};

/// A stage that rewrites volatile text before comparison. The default only
/// replaces the run's own directories; the full normaliser (UUIDs, timestamps,
/// shas, ...) is CR-02 and plugs in here.
pub trait Normaliser: Sync {
    fn normalise(&self, run: &RunResult, text: &str) -> String;
}

/// Replaces the sandbox directories, in both slash styles.
pub struct RootsNormaliser;

impl Normaliser for RootsNormaliser {
    fn normalise(&self, run: &RunResult, text: &str) -> String {
        let mut out = text.to_owned();
        for (root, placeholder) in &run.roots {
            out = replace_root(&out, root, placeholder);
            out = replace_root(&out, &root.replace('\\', "/"), placeholder);
            out = replace_root(&out, &root.replace('\\', "\\\\"), placeholder);
        }
        out
    }
}

/// Replaces each occurrence of `root` that ends a path (see [`ends_a_path`]). `/w/sb/home` names
/// `/w/sb/home/.wg` but not `/w/sb/home2` or `/w/sb/home.bak`, which are other directories. An
/// occurrence that is not at a path boundary is kept as it is.
fn replace_root(text: &str, root: &str, placeholder: &str) -> String {
    if root.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(root) {
        let after = &rest[at + root.len()..];
        out.push_str(&rest[..at]);
        if ends_a_path(after) {
            out.push_str(placeholder);
        } else {
            out.push_str(root);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Whether a root that is followed by `after` ends a path: nothing follows, or the next character
/// cannot continue a name. A separator (`/`, `\`) ends it, and so does a quote, a space or other
/// punctuation, which the paths in quoted JSON and in messages are followed by. A letter, a digit,
/// `_`, `-` or `.` continues the name.
fn ends_a_path(after: &str) -> bool {
    after
        .chars()
        .next()
        .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.')))
}

/// One allowlist entry from `divergences.toml`.
#[derive(Debug, Clone)]
pub struct Divergence {
    pub id: String,
    pub decision: String,
    /// Scenario id, or `*`.
    pub scenario: String,
    /// Step index, or any step when absent.
    pub step: Option<usize>,
    /// `stdout`, `stderr`, `exit`; all of them when empty.
    pub fields: Vec<String>,
    /// Substrings, one of which must occur in the left or the right text of the difference.
    /// Empty means any text. A divergence names the cases it allows through these, so an entry
    /// for one kind of output does not hide differences in other output.
    pub contains: Vec<String>,
    /// With `contains`: the substring must occur on both sides of the difference, not on one.
    /// An entry for help text needs the help text on both sides, so a config error that happens
    /// to name `Usage:` on one side is not allowed.
    pub both: bool,
    /// When set, the left or the right text of the difference holds a non-ASCII character.
    pub non_ascii: bool,
    /// When set, the two texts must be identical after tokenising numbers, and every pair of
    /// differing numbers must be floats (a `.` or an exponent) within this relative tolerance.
    /// Differing integers, differing words and a differing token count are not allowed.
    pub relative_tolerance: Option<f64>,
}

/// One piece of a text: a run of non-numeric text, or a number.
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Text(&'a str),
    Num { text: &'a str, float: bool },
}

/// Split `s` into text runs and number tokens (`-`? digits, optional fraction, optional exponent).
/// A `-` belongs to a number only when it is not glued to a preceding word character.
fn pieces(s: &str) -> Vec<Piece<'_>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut text_start) = (0, 0);
    while i < b.len() {
        let neg = b[i] == b'-'
            && b.get(i + 1).is_some_and(u8::is_ascii_digit)
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'));
        if !(b[i].is_ascii_digit() || neg) {
            i += 1;
            continue;
        }
        let start = i;
        i += usize::from(neg);
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let mut float = false;
        if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            float = true;
            i += 1;
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        if matches!(b.get(i), Some(b'e' | b'E')) {
            let sign = usize::from(matches!(b.get(i + 1), Some(b'+' | b'-')));
            if b.get(i + 1 + sign).is_some_and(u8::is_ascii_digit) {
                float = true;
                i += 1 + sign;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
            }
        }
        if start > text_start {
            out.push(Piece::Text(&s[text_start..start]));
        }
        out.push(Piece::Num {
            text: &s[start..i],
            float,
        });
        text_start = i;
    }
    if text_start < b.len() {
        out.push(Piece::Text(&s[text_start..]));
    }
    out
}

/// True when `left` and `right` differ only in float tokens that agree within `tol`
/// (`|a-b| <= tol * max(|a|,|b|)`).
pub fn within_relative_tolerance(left: &str, right: &str, tol: f64) -> bool {
    let (l, r) = (pieces(left), pieces(right));
    l.len() == r.len()
        && l.iter().zip(&r).all(|(a, b)| match (a, b) {
            (Piece::Text(x), Piece::Text(y)) => x == y,
            (Piece::Num { text: x, float: fx }, Piece::Num { text: y, float: fy }) => {
                x == y
                    || (*fx
                        && *fy
                        && matches!(
                            (x.parse::<f64>(), y.parse::<f64>()),
                            (Ok(a), Ok(b)) if (a - b).abs() <= tol * a.abs().max(b.abs())
                        ))
            }
            _ => false,
        })
}

impl Divergence {
    fn covers(
        &self,
        scenario: &str,
        step: Option<usize>,
        field: &str,
        left: &str,
        right: &str,
    ) -> bool {
        let names = |s: &String| {
            if self.both {
                left.contains(s.as_str()) && right.contains(s.as_str())
            } else {
                left.contains(s.as_str()) || right.contains(s.as_str())
            }
        };
        let text_ok = self.contains.is_empty() || self.contains.iter().any(names);
        let encoding_ok = !self.non_ascii || !left.is_ascii() || !right.is_ascii();
        let tolerance_ok = self
            .relative_tolerance
            .is_none_or(|tol| within_relative_tolerance(left, right, tol));
        (self.scenario == "*" || self.scenario == scenario)
            && (self.step.is_none() || self.step == step)
            && (self.fields.is_empty() || self.fields.iter().any(|f| f == field))
            && text_ok
            && encoding_ok
            && tolerance_ok
    }
}

/// Parse `[[divergence]]` tables. Every entry must name its `id` and the `decision` that allows it.
pub fn parse_divergences(text: &str) -> Result<Vec<Divergence>, String> {
    let doc: toml::Table = text.parse().map_err(|e| format!("divergences: {e}"))?;
    let Some(list) = doc.get("divergence") else {
        return Ok(Vec::new());
    };
    let list = list
        .as_array()
        .ok_or("divergences: `divergence` must be an array of tables")?;
    list.iter()
        .enumerate()
        .map(|(i, t)| {
            let t = t
                .as_table()
                .ok_or_else(|| format!("divergence[{i}]: expected a table"))?;
            let s = |k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_owned);
            let need = |k: &str| s(k).ok_or_else(|| format!("divergence[{i}]: missing `{k}`"));
            Ok(Divergence {
                id: need("id")?,
                decision: need("decision")?,
                scenario: s("scenario").unwrap_or_else(|| "*".to_owned()),
                step: t
                    .get("step")
                    .and_then(toml::Value::as_integer)
                    .and_then(|n| usize::try_from(n).ok()),
                fields: t
                    .get("fields")
                    .and_then(toml::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                contains: strings_of(t, "contains"),
                both: t
                    .get("both")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
                non_ascii: t
                    .get("non_ascii")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
                relative_tolerance: match t.get("relative_tolerance") {
                    None => None,
                    Some(v) => Some(
                        v.as_float()
                            .filter(|x| x.is_finite() && *x > 0.0)
                            .ok_or_else(|| {
                                format!(
                                    "divergence[{i}]: `relative_tolerance` must be a positive float"
                                )
                            })?,
                    ),
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .map(|d| {
            if d.scenario == "*"
                && d.contains.is_empty()
                && !d.non_ascii
                && d.relative_tolerance.is_none()
            {
                Err(format!(
                    "divergence {}: scenario \"*\" needs `contains`, `non_ascii` or `relative_tolerance` (a wildcard that allows everything is illegal)",
                    d.id
                ))
            } else {
                Ok(d)
            }
        })
        .collect()
}

/// The string elements of an array-valued key, empty when the key is absent.
fn strings_of(t: &toml::Table, key: &str) -> Vec<String> {
    t.get(key)
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The comparable part of a step, after normalisation.
#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    pub index: Option<usize>,
    pub kind: String,
    pub exit: Option<i64>,
    pub stdout: String,
    pub stderr: String,
}

/// Normalised observations of a run, in step order.
pub fn observe(run: &RunResult, n: &dyn Normaliser) -> Vec<Observed> {
    run.steps
        .iter()
        .map(|s| Observed {
            index: s.index,
            kind: s.kind.to_owned(),
            exit: s.exit,
            stdout: n.normalise(run, &s.stdout),
            stderr: n.normalise(run, &s.stderr),
        })
        .collect()
}

/// The golden file for a scenario.
pub fn golden_json(scenario: &str, obs: &[Observed]) -> Value {
    json!({
        "scenario": scenario,
        "steps": obs.iter().map(|o| json!({
            "index": o.index, "kind": o.kind, "exit": o.exit, "stdout": o.stdout, "stderr": o.stderr,
        })).collect::<Vec<_>>(),
    })
}

/// Read a golden file back.
pub fn parse_golden(text: &str) -> Result<Vec<Observed>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("golden: {e}"))?;
    let steps = v
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("golden: missing `steps`")?;
    Ok(steps
        .iter()
        .map(|s| Observed {
            index: s
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|n| usize::try_from(n).ok()),
            kind: s
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            exit: s.get("exit").and_then(Value::as_i64),
            stdout: s
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            stderr: s
                .get("stderr")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
        .collect())
}

/// One difference between two observation lists.
#[derive(Debug, Clone)]
pub struct Diff {
    pub step: Option<usize>,
    pub field: &'static str,
    pub left: String,
    pub right: String,
    /// The divergence id that allows it, when one does.
    pub allowed_by: Option<String>,
}

/// Compare `left` with `right`, marking differences the allowlist covers.
pub fn compare(
    scenario: &str,
    left: &[Observed],
    right: &[Observed],
    allow: &[Divergence],
) -> Vec<Diff> {
    let mut diffs = Vec::new();
    let mut push = |step: Option<usize>, field: &'static str, l: String, r: String| {
        let allowed_by = allow
            .iter()
            .find(|d| d.covers(scenario, step, field, &l, &r))
            .map(|d| d.id.clone());
        diffs.push(Diff {
            step,
            field,
            left: l,
            right: r,
            allowed_by,
        });
    };
    if left.len() != right.len() {
        push(
            None,
            "step_count",
            left.len().to_string(),
            right.len().to_string(),
        );
    }
    for (l, r) in left.iter().zip(right) {
        if l.exit != r.exit {
            push(
                l.index,
                "exit",
                format!("{:?}", l.exit),
                format!("{:?}", r.exit),
            );
        }
        if l.stdout != r.stdout {
            push(l.index, "stdout", l.stdout.clone(), r.stdout.clone());
        }
        if l.stderr != r.stderr {
            push(l.index, "stderr", l.stderr.clone(), r.stderr.clone());
        }
    }
    diffs
}

/// Replace bare normaliser placeholders (`<dur>`, `<id:1>`) outside string literals with string
/// literals, so a numeric field the normaliser masked still parses as JSON.
fn quote_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if c == '<' {
            out.push_str("\"<");
            for d in chars.by_ref() {
                if d == '"' || d == '\\' {
                    continue;
                }
                out.push(d);
                if d == '>' {
                    break;
                }
            }
            out.push('"');
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a string value holds a normaliser placeholder such as `<id:1>` or `<dur>`.
fn has_placeholder(s: &str) -> bool {
    s.find('<').is_some_and(|i| s[i..].contains('>'))
}

fn kind_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Describe where two JSON values differ in shape: object keys, value kinds, array lengths.
/// Scalar values are not compared, and a placeholder string stands for any scalar.
fn shape_difference(path: &str, l: &Value, r: &Value) -> Option<String> {
    let placeholder = |v: &Value| v.as_str().is_some_and(has_placeholder);
    if placeholder(l) || placeholder(r) {
        return None;
    }
    if kind_name(l) != kind_name(r) {
        return Some(format!("{path}: {} vs {}", kind_name(l), kind_name(r)));
    }
    match (l, r) {
        (Value::Object(a), Value::Object(b)) => {
            if let Some(k) = a.keys().find(|k| !b.contains_key(*k)) {
                return Some(format!("{path}.{k}: key only in the golden"));
            }
            if let Some(k) = b.keys().find(|k| !a.contains_key(*k)) {
                return Some(format!("{path}.{k}: key only in the run"));
            }
            a.iter()
                .find_map(|(k, v)| shape_difference(&format!("{path}.{k}"), v, &b[k]))
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: array length {} vs {}", a.len(), b.len()));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(i, (x, y))| shape_difference(&format!("{path}[{i}]"), x, y))
        }
        _ => None,
    }
}

/// Compare the behaviour of two runs, for `check`: step count, exit code (the HTTP status for
/// HTTP steps), and the shape of stdout. Scalar values and stderr are not compared.
///
/// When both stdouts parse as JSON (placeholders count as wildcards), their shapes are compared:
/// the same object keys recursively, the same value kinds and the same array lengths. Otherwise
/// only whether each is empty is compared.
pub fn compare_behaviour(
    scenario: &str,
    left: &[Observed],
    right: &[Observed],
    allow: &[Divergence],
) -> Vec<Diff> {
    let mut diffs = Vec::new();
    let mut push = |step: Option<usize>, field: &'static str, l: String, r: String| {
        let allowed_by = allow
            .iter()
            .find(|d| d.covers(scenario, step, field, &l, &r))
            .map(|d| d.id.clone());
        diffs.push(Diff {
            step,
            field,
            left: l,
            right: r,
            allowed_by,
        });
    };
    if left.len() != right.len() {
        push(
            None,
            "step_count",
            left.len().to_string(),
            right.len().to_string(),
        );
    }
    let parse = |t: &str| serde_json::from_str::<Value>(&quote_placeholders(t)).ok();
    let word = |t: &str| {
        if t.trim().is_empty() {
            "empty"
        } else {
            "non-empty"
        }
        .to_owned()
    };
    for (l, r) in left.iter().zip(right) {
        if l.exit != r.exit {
            push(
                l.index,
                "exit",
                format!("{:?}", l.exit),
                format!("{:?}", r.exit),
            );
        }
        match (parse(&l.stdout), parse(&r.stdout)) {
            (Some(a), Some(b)) => {
                if let Some(why) = shape_difference("$", &a, &b) {
                    push(
                        l.index,
                        "stdout",
                        format!("shape differs: {why}"),
                        "(see left)".to_owned(),
                    );
                }
            }
            _ => {
                if word(&l.stdout) != word(&r.stdout) {
                    push(l.index, "stdout", word(&l.stdout), word(&r.stdout));
                }
            }
        }
    }
    diffs
}

/// The per-file verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Equal,
    Allowlisted,
    Failed,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Equal => "equal",
            Verdict::Allowlisted => "allowlisted",
            Verdict::Failed => "failed",
        }
    }
}

/// A step that took longer than [`SLOW_STEP_MICROS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlowStep {
    /// The implementation that ran the step.
    pub implementation: String,
    /// The step index, or `None` for a setup step.
    pub index: Option<usize>,
    pub kind: &'static str,
    pub ms: u64,
}

/// Steps slower than this are listed in a report (one second).
pub const SLOW_STEP_MICROS: i64 = 1_000_000;

/// The report line for one scenario file.
#[derive(Debug, Clone)]
pub struct FileReport {
    pub file: String,
    pub scenario: Option<String>,
    pub verdict: Verdict,
    pub diffs: Vec<Diff>,
    pub problems: Vec<String>,
    /// Wall-clock time the file took in its mode, every implementation's run included.
    pub duration_ms: u64,
    /// The steps of every run that took longer than a second, in run and step order.
    pub slow_steps: Vec<SlowStep>,
}

impl FileReport {
    pub fn failed(file: &str, scenario: Option<String>, problem: String) -> Self {
        Self {
            file: file.to_owned(),
            scenario,
            verdict: Verdict::Failed,
            diffs: Vec::new(),
            problems: vec![problem],
            duration_ms: 0,
            slow_steps: Vec::new(),
        }
    }

    /// Decide the verdict from diffs and failed steps (`runs` are every run that took part).
    pub fn judge(file: &str, scenario: &str, diffs: Vec<Diff>, runs: &[&RunResult]) -> Self {
        let mut problems = Vec::new();
        for run in runs {
            for s in run.steps.iter().filter(|s| s.status == Status::Failed) {
                let at = s.index.map_or("setup".to_owned(), |i| format!("step {i}"));
                problems.push(format!(
                    "{} {at} ({}): {}",
                    run.implementation, s.label, s.note
                ));
            }
        }
        let verdict = if !problems.is_empty() || diffs.iter().any(|d| d.allowed_by.is_none()) {
            Verdict::Failed
        } else if diffs.is_empty() {
            Verdict::Equal
        } else {
            Verdict::Allowlisted
        };
        let slow_steps = runs
            .iter()
            .flat_map(|run| {
                run.steps.iter().filter_map(|s| {
                    let (start, end) = s.window?;
                    (end - start > SLOW_STEP_MICROS).then(|| SlowStep {
                        implementation: run.implementation.clone(),
                        index: s.index,
                        kind: s.kind,
                        ms: u64::try_from((end - start) / 1000).unwrap_or(0),
                    })
                })
            })
            .collect();
        Self {
            file: file.to_owned(),
            scenario: Some(scenario.to_owned()),
            verdict,
            diffs,
            problems,
            duration_ms: 0,
            slow_steps,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "file": self.file, "scenario": self.scenario, "verdict": self.verdict.as_str(),
            "duration_ms": self.duration_ms,
            "slow_steps": self.slow_steps.iter().map(|s| json!({
                "implementation": s.implementation, "index": s.index, "kind": s.kind, "ms": s.ms,
            })).collect::<Vec<_>>(),
            "problems": self.problems,
            "diffs": self.diffs.iter().map(|d| json!({
                "step": d.step, "field": d.field, "left": d.left, "right": d.right, "allowed_by": d.allowed_by,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Counts per verdict.
pub fn tally(reports: &[FileReport]) -> (usize, usize, usize) {
    let n = |v: Verdict| reports.iter().filter(|r| r.verdict == v).count();
    (
        n(Verdict::Equal),
        n(Verdict::Allowlisted),
        n(Verdict::Failed),
    )
}

/// The report as JSON.
pub fn report_json(mode: &str, reports: &[FileReport]) -> Value {
    let (equal, allowlisted, failed) = tally(reports);
    json!({
        "mode": mode,
        "summary": {"equal": equal, "allowlisted": allowlisted, "failed": failed},
        "files": reports.iter().map(FileReport::to_json).collect::<Vec<_>>(),
    })
}

/// The report as text, one line per file plus detail lines for failures.
pub fn report_text(mode: &str, reports: &[FileReport]) -> String {
    let mut out = String::new();
    for r in reports {
        out.push_str(&format!(
            "{:<11} {} ({} ms)\n",
            r.verdict.as_str(),
            r.file,
            r.duration_ms
        ));
        for s in &r.slow_steps {
            let at = s.index.map_or("setup".to_owned(), |i| format!("step {i}"));
            out.push_str(&format!(
                "    slow {} {at} ({}): {} ms\n",
                s.implementation, s.kind, s.ms
            ));
        }
        for p in &r.problems {
            out.push_str(&format!("    {p}\n"));
        }
        for d in &r.diffs {
            let step = d.step.map_or("-".to_owned(), |s| s.to_string());
            let tag = d
                .allowed_by
                .as_deref()
                .map_or(String::new(), |id| format!(" [allowed by {id}]"));
            out.push_str(&format!(
                "    step {step} {}: {:?} != {:?}{tag}\n",
                d.field,
                clip(&d.left),
                clip(&d.right)
            ));
        }
    }
    let (equal, allowlisted, failed) = tally(reports);
    out.push_str(&format!(
        "{mode}: {equal} equal, {allowlisted} allowlisted, {failed} failed\n"
    ));
    out
}

fn clip(s: &str) -> String {
    if s.chars().count() > 200 {
        format!("{}...", s.chars().take(200).collect::<String>())
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(stdout: &str) -> Vec<Observed> {
        vec![Observed {
            index: Some(0),
            kind: "cli".into(),
            exit: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
        }]
    }

    fn obs_exit(exit: i64, stdout: &str) -> Vec<Observed> {
        let mut o = obs(stdout);
        o[0].exit = Some(exit);
        o
    }

    #[test]
    fn behaviour_ignores_scalar_values_and_stderr() {
        let a = obs(r#"{"id":"WG-1","n":3,"tags":["a","b"],"x":null}"#);
        let mut b = obs(r#"{"id":"WG-9","n":40,"tags":["c","d"],"x":null}"#);
        b[0].stderr = "noise".into();
        assert!(compare_behaviour("s", &a, &b, &[]).is_empty());
    }

    #[test]
    fn behaviour_compares_keys_kinds_lengths_and_exit() {
        let base = obs(r#"{"a":1,"l":[1,2]}"#);
        for other in [
            r#"{"a":1}"#,
            r#"{"a":1,"l":[1,2],"z":0}"#,
            r#"{"a":"1","l":[1,2]}"#,
            r#"{"a":1,"l":[1]}"#,
        ] {
            let d = compare_behaviour("s", &base, &obs(other), &[]);
            assert_eq!(d.len(), 1, "{other}");
            assert_eq!(d[0].field, "stdout");
        }
        let d = compare_behaviour("s", &base, &obs_exit(2, r#"{"a":1,"l":[1,2]}"#), &[]);
        assert_eq!(d[0].field, "exit");
    }

    #[test]
    fn behaviour_treats_placeholders_as_wildcards() {
        let a = obs(r#"{"ms": <dur>, "id":"<id:1>", "n": 5}"#);
        let b = obs(r#"{"ms": 12, "id": 7, "n": 6}"#);
        assert!(compare_behaviour("s", &a, &b, &[]).is_empty());
    }

    #[test]
    fn behaviour_on_text_compares_only_emptiness() {
        assert!(compare_behaviour("s", &obs("hello\n"), &obs("other text"), &[]).is_empty());
        assert_eq!(
            compare_behaviour("s", &obs("hello"), &obs("\n"), &[]).len(),
            1
        );
        assert_eq!(compare_behaviour("s", &obs("{}"), &obs(""), &[]).len(), 1);
    }

    #[test]
    fn roots_are_replaced_only_at_a_path_boundary() {
        let run = |root: &str| RunResult {
            scenario: "s".into(),
            implementation: "rs".into(),
            roots: vec![(root.into(), "<home>")],
            steps: Vec::new(),
            started: None,
        };
        // POSIX form: a root followed by a separator, a quote, a space or the end is replaced; a
        // longer name (`home2`, `homes`, `home.bak`) is not.
        let posix = run("/w/sb/home");
        assert_eq!(
            RootsNormaliser.normalise(
                &posix,
                r#"/w/sb/home/.wg /w/sb/home2/.wg /w/sb/home /w/sb/home\x /w/sb/homes "/w/sb/home" /w/sb/home.bak"#
            ),
            r#"<home>/.wg /w/sb/home2/.wg <home> <home>\x /w/sb/homes "<home>" /w/sb/home.bak"#
        );
        // Windows form, its slash-normalised copy and its JSON-escaped copy, each at a boundary.
        let windows = run(r"C:\w\sb\home");
        assert_eq!(
            RootsNormaliser.normalise(
                &windows,
                r"C:\w\sb\home\.wg C:\w\sb\home2\.wg C:/w/sb/home/x C:\\w\\sb\\home\\x C:\\w\\sb\\home2"
            ),
            r"<home>\.wg C:\w\sb\home2\.wg <home>/x <home>\\x C:\\w\\sb\\home2"
        );
    }

    #[test]
    fn allowlist_turns_failure_into_allowlisted() {
        let allow = parse_divergences("[[divergence]]\nid = \"DV2\"\ndecision = \"D2\"\nscenario = \"s\"\nfields = [\"stdout\"]\n").unwrap();
        let d = compare("s", &obs("a"), &obs("b"), &allow);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].allowed_by.as_deref(), Some("DV2"));
        assert!(
            compare("other", &obs("a"), &obs("b"), &allow)[0]
                .allowed_by
                .is_none()
        );
        assert!(compare("s", &obs("a"), &obs("a"), &allow).is_empty());
    }

    #[test]
    fn allowlist_entries_match_only_the_text_they_name() {
        let allow = parse_divergences(
            "[[divergence]]\nid = \"DV1\"\ndecision = \"Q1\"\nfields = [\"stdout\"]\ncontains = [\"Usage:\"]\n\n\
             [[divergence]]\nid = \"DV2\"\ndecision = \"Q2\"\nfields = [\"stdout\"]\nnon_ascii = true\n",
        )
        .unwrap();
        // Help output is allowed by DV1, whichever side holds the text.
        let help = compare(
            "s",
            &obs("Usage: wg [OPTIONS]\n"),
            &obs("usage: wg\n"),
            &allow,
        );
        assert_eq!(help[0].allowed_by.as_deref(), Some("DV1"));
        // A difference in another output is not allowed by DV1.
        let other = compare("s", &obs("task WG-1\n"), &obs("task WG-2\n"), &allow);
        assert!(other[0].allowed_by.is_none());
        // Raw attempt ids are not help output, so they are not hidden either.
        let ids = compare(
            "s",
            &obs("wg/WG-1/attempt-1-1157221c\n"),
            &obs("wg/WG-1/attempt-1-d3b95462\n"),
            &allow,
        );
        assert!(ids[0].allowed_by.is_none());
        // Encoding differences are allowed only when one side is not ASCII.
        assert_eq!(
            compare("s", &obs("é\n"), &obs("?\n"), &allow)[0]
                .allowed_by
                .as_deref(),
            Some("DV2")
        );
        assert!(
            compare("s", &obs("a\n"), &obs("b\n"), &allow)[0]
                .allowed_by
                .is_none()
        );
    }

    #[test]
    fn both_sides_entries_need_the_text_on_each_side() {
        let allow = parse_divergences(
            "[[divergence]]\nid = \"DV1\"\ndecision = \"Q1\"\nfields = [\"stdout\"]\ncontains = [\"Usage:\"]\nboth = true\n",
        )
        .unwrap();
        // Help on both sides: allowed.
        let help = compare(
            "s",
            &obs("Usage: wg [OPTIONS]\n"),
            &obs("Usage: wg\n"),
            &allow,
        );
        assert_eq!(help[0].allowed_by.as_deref(), Some("DV1"));
        // A config error on one side that names `Usage:` is not help text on both sides.
        let config = compare(
            "s",
            &obs("Usage: wg\n"),
            &obs("error: bad config\n"),
            &allow,
        );
        assert!(config[0].allowed_by.is_none());
    }

    #[test]
    fn tolerance_allows_close_floats_only() {
        let w = within_relative_tolerance;
        assert!(w(
            "mean=1.0000000000001 n=3\n",
            "mean=1.0000000000002 n=3\n",
            1e-12
        ));
        assert!(w("x=1.5e-3", "x=1.5000000000001e-3", 1e-12));
        assert!(!w("mean=1.001\n", "mean=1.002\n", 1e-12));
        assert!(w("a 2.5 b", "a 2.5 b", 1e-12));
    }

    #[test]
    fn tolerance_rejects_integers_words_and_shape() {
        let w = within_relative_tolerance;
        assert!(!w("failed=3", "failed=0", 1e-12));
        assert!(!w("n=1000000000000", "n=1000000000001", 1e-12));
        assert!(!w("mean=1.0 ok", "mean=1.0 bad", 1e-12));
        assert!(!w("1.0 2.0", "1.0", 1e-12));
        assert!(!w("v=1", "v=1.0", 1e-12));
        assert!(!w("WG-1", "WG-2", 1e-12));
    }

    #[test]
    fn tolerance_entry_applies_through_compare() {
        let allow = parse_divergences(
            "[[divergence]]\nid = \"DV7\"\ndecision = \"Q9\"\nfields = [\"stdout\"]\nrelative_tolerance = 1e-12\n",
        )
        .unwrap();
        let near = compare(
            "s",
            &obs("p=0.30000000000000004\n"),
            &obs("p=0.3\n"),
            &allow,
        );
        assert_eq!(near[0].allowed_by.as_deref(), Some("DV7"));
        let far = compare("s", &obs("failed=3\n"), &obs("failed=0\n"), &allow);
        assert!(far[0].allowed_by.is_none());
    }

    #[test]
    fn wildcard_entry_must_name_what_it_allows() {
        let bare = "[[divergence]]\nid = \"X\"\ndecision = \"Q\"\nscenario = \"*\"\nfields = [\"stdout\"]\n";
        assert!(parse_divergences(bare).is_err());
        for extra in [
            "contains = [\"a\"]",
            "non_ascii = true",
            "relative_tolerance = 1e-9",
        ] {
            assert!(parse_divergences(&format!("{bare}{extra}\n")).is_ok());
        }
        // A scenario-specific entry may stay bare.
        assert!(parse_divergences(&bare.replace("\"*\"", "\"s\"")).is_ok());
    }

    #[test]
    fn divergence_needs_a_decision() {
        assert!(parse_divergences("[[divergence]]\nid = \"DV1\"\n").is_err());
        assert!(parse_divergences("").unwrap().is_empty());
    }

    #[test]
    fn golden_round_trips() {
        let o = obs("hello\n");
        let text = golden_json("s", &o).to_string();
        assert_eq!(parse_golden(&text).unwrap(), o);
    }
}
