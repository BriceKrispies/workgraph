//! Comparing runs, the divergence allowlist, goldens and the per-file report.

use crate::runner::{RunResult, Status};
use serde_json::{Value, json};

/// A stage that rewrites volatile text before comparison. The default only
/// replaces the run's own directories; the full normaliser (UUIDs, timestamps,
/// shas, ...) is CR-02 and plugs in here.
pub trait Normaliser {
    fn normalise(&self, run: &RunResult, text: &str) -> String;
}

/// Replaces the sandbox directories, in both slash styles.
pub struct RootsNormaliser;

impl Normaliser for RootsNormaliser {
    fn normalise(&self, run: &RunResult, text: &str) -> String {
        let mut out = text.to_owned();
        for (root, placeholder) in &run.roots {
            out = out.replace(root.as_str(), placeholder);
            out = out.replace(&root.replace('\\', "/"), placeholder);
            out = out.replace(&root.replace('\\', "\\\\"), placeholder);
        }
        out
    }
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
}

impl Divergence {
    fn covers(&self, scenario: &str, step: Option<usize>, field: &str) -> bool {
        (self.scenario == "*" || self.scenario == scenario)
            && (self.step.is_none() || self.step == step)
            && (self.fields.is_empty() || self.fields.iter().any(|f| f == field))
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
            })
        })
        .collect()
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
            .find(|d| d.covers(scenario, step, field))
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

/// The report line for one scenario file.
#[derive(Debug, Clone)]
pub struct FileReport {
    pub file: String,
    pub scenario: Option<String>,
    pub verdict: Verdict,
    pub diffs: Vec<Diff>,
    pub problems: Vec<String>,
}

impl FileReport {
    pub fn failed(file: &str, scenario: Option<String>, problem: String) -> Self {
        Self {
            file: file.to_owned(),
            scenario,
            verdict: Verdict::Failed,
            diffs: Vec::new(),
            problems: vec![problem],
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
        Self {
            file: file.to_owned(),
            scenario: Some(scenario.to_owned()),
            verdict,
            diffs,
            problems,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "file": self.file, "scenario": self.scenario, "verdict": self.verdict.as_str(),
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
        out.push_str(&format!("{:<11} {}\n", r.verdict.as_str(), r.file));
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
