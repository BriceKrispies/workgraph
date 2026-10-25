//! The three modes: `diff`, `record` and `check`.

use crate::compare::{
    Divergence, FileReport, Normaliser, compare, golden_json, observe, parse_golden,
};
use crate::runner::{Implementation, RunResult, run_scenario};
use crate::scenario::{Scenario, parse_scenario};
use std::path::{Path, PathBuf};

/// A scenario file that loaded (or failed to).
pub struct Loaded {
    pub file: String,
    pub scenario: Result<Scenario, String>,
}

/// Load every `*.json` file of a directory (sorted), or one file.
pub fn load_scenarios(path: &Path) -> Result<Vec<Loaded>, String> {
    let mut files: Vec<PathBuf> = if path.is_dir() {
        std::fs::read_dir(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect()
    } else {
        vec![path.to_path_buf()]
    };
    files.sort();
    Ok(files
        .into_iter()
        .map(|p| {
            let file = p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let scenario = std::fs::read_to_string(&p)
                .map_err(|e| format!("read: {e}"))
                .and_then(|t| parse_scenario(&t))
                .and_then(|s| {
                    let stem = file.strip_suffix(".json").unwrap_or(&file);
                    if s.id == stem {
                        Ok(s)
                    } else {
                        Err(format!("id `{}` must equal the file name `{stem}`", s.id))
                    }
                });
            Loaded { file, scenario }
        })
        .collect())
}

/// Write a run's step log to `<out>/<implementation>/<scenario>.log.json`.
fn write_log(out: Option<&Path>, run: &RunResult) -> Result<(), String> {
    let Some(out) = out else { return Ok(()) };
    let dir = out.join(&run.implementation);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let mut text = serde_json::to_string_pretty(&run.log_json()).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(dir.join(format!("{}.log.json", run.scenario)), text).map_err(|e| e.to_string())
}

/// `diff`: run the reference and candidate implementations and compare them.
pub fn diff(
    loaded: &[Loaded],
    reference: &Implementation,
    candidate: &Implementation,
    allow: &[Divergence],
    norm: &dyn Normaliser,
    out: Option<&Path>,
) -> Vec<FileReport> {
    loaded
        .iter()
        .map(|l| {
            let s = match &l.scenario {
                Ok(s) => s,
                Err(e) => {
                    return FileReport::failed(&l.file, None, format!("invalid scenario: {e}"));
                }
            };
            let run = |imp| run_scenario(s, imp).and_then(|r| write_log(out, &r).map(|()| r));
            match (run(reference), run(candidate)) {
                (Ok(a), Ok(b)) => {
                    let diffs = compare(&s.id, &observe(&a, norm), &observe(&b, norm), allow);
                    FileReport::judge(&l.file, &s.id, diffs, &[&a, &b])
                }
                (Err(e), _) | (_, Err(e)) => FileReport::failed(&l.file, Some(s.id.clone()), e),
            }
        })
        .collect()
}

/// `record`: run the reference implementation and write goldens to `golden_dir`.
pub fn record(
    loaded: &[Loaded],
    reference: &Implementation,
    golden_dir: &Path,
    norm: &dyn Normaliser,
    out: Option<&Path>,
) -> Vec<FileReport> {
    loaded
        .iter()
        .map(|l| {
            let s = match &l.scenario {
                Ok(s) => s,
                Err(e) => {
                    return FileReport::failed(&l.file, None, format!("invalid scenario: {e}"));
                }
            };
            let result = run_scenario(s, reference).and_then(|r| {
                write_log(out, &r)?;
                std::fs::create_dir_all(golden_dir)
                    .map_err(|e| format!("create {}: {e}", golden_dir.display()))?;
                let mut text =
                    serde_json::to_string_pretty(&golden_json(&s.id, &observe(&r, norm)))
                        .map_err(|e| e.to_string())?;
                text.push('\n');
                std::fs::write(golden_dir.join(format!("{}.json", s.id)), text)
                    .map_err(|e| e.to_string())?;
                Ok(r)
            });
            match result {
                Ok(r) => FileReport::judge(&l.file, &s.id, Vec::new(), &[&r]),
                Err(e) => FileReport::failed(&l.file, Some(s.id.clone()), e),
            }
        })
        .collect()
}

/// `check`: run the candidate implementation and compare it with the goldens.
pub fn check(
    loaded: &[Loaded],
    candidate: &Implementation,
    golden_dir: &Path,
    allow: &[Divergence],
    norm: &dyn Normaliser,
    out: Option<&Path>,
) -> Vec<FileReport> {
    loaded
        .iter()
        .map(|l| {
            let s = match &l.scenario {
                Ok(s) => s,
                Err(e) => {
                    return FileReport::failed(&l.file, None, format!("invalid scenario: {e}"));
                }
            };
            let golden_path = golden_dir.join(format!("{}.json", s.id));
            let golden = match std::fs::read_to_string(&golden_path) {
                Ok(t) => match parse_golden(&t) {
                    Ok(g) => g,
                    Err(e) => return FileReport::failed(&l.file, Some(s.id.clone()), e),
                },
                Err(e) => {
                    return FileReport::failed(
                        &l.file,
                        Some(s.id.clone()),
                        format!("no golden {}: {e}", golden_path.display()),
                    );
                }
            };
            match run_scenario(s, candidate).and_then(|r| write_log(out, &r).map(|()| r)) {
                Ok(r) => {
                    let diffs = compare(&s.id, &golden, &observe(&r, norm), allow);
                    FileReport::judge(&l.file, &s.id, diffs, &[&r])
                }
                Err(e) => FileReport::failed(&l.file, Some(s.id.clone()), e),
            }
        })
        .collect()
}
