//! The three modes: `diff`, `record` and `check`.

use crate::compare::{
    Divergence, FileReport, Normaliser, compare, golden_json, observe, parse_golden,
};
use crate::runner::{Implementation, RunResult, run_scenario};
use crate::scenario::{Scenario, parse_scenario};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Instant;

/// A scenario file that loaded (or failed to).
pub struct Loaded {
    pub file: String,
    pub scenario: Result<Scenario, String>,
}

/// Load every `*.json` file under a directory, recursively (so `scenarios/<area>/` is loaded), or
/// one file. Files are sorted by their path relative to the directory, which is also the name
/// a report gives them.
pub fn load_scenarios(path: &Path) -> Result<Vec<Loaded>, String> {
    let mut files: Vec<PathBuf> = Vec::new();
    if path.is_dir() {
        collect_json(path, &mut files)?;
    } else {
        files.push(path.to_path_buf());
    }
    files.sort();
    Ok(files
        .into_iter()
        .map(|p| {
            let file = if path.is_dir() {
                p.strip_prefix(path)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/")
            } else {
                display_name(&p)
            };
            let scenario = std::fs::read_to_string(&p)
                .map_err(|e| format!("read: {e}"))
                .and_then(|t| parse_scenario(&t))
                .and_then(|s| {
                    let name = display_name(&p);
                    let stem = name.strip_suffix(".json").unwrap_or(&name);
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

/// The last component of a scenario path, for error messages and the id check.
fn display_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Collect the `*.json` files under `dir`, descending into subdirectories.
fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            collect_json(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(p);
        }
    }
    Ok(())
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

/// Run `work` on every item with up to `jobs` worker threads and return the results in the items'
/// order, whatever order the workers finish in. Workers claim the next unclaimed index from a shared
/// counter, so a slow scenario does not hold up the scenarios queued behind it on other workers.
pub fn run_in_order<I: Sync, T: Send>(
    items: &[I],
    jobs: usize,
    work: impl Fn(&I) -> T + Sync,
) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let done = Mutex::new(Vec::with_capacity(items.len()));
    let workers = jobs.clamp(1, items.len().max(1));
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else { break };
                    let result = work(item);
                    done.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((index, result));
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap_or_else(PoisonError::into_inner);
    done.sort_unstable_by_key(|(index, _)| *index);
    done.into_iter().map(|(_, result)| result).collect()
}

/// Judge every loaded file with `work` on `jobs` workers, in file order, and stamp each report with
/// the wall-clock time its work took.
fn per_file(
    loaded: &[Loaded],
    jobs: usize,
    work: impl Fn(&Loaded) -> FileReport + Sync,
) -> Vec<FileReport> {
    run_in_order(loaded, jobs, |l| {
        let started = Instant::now();
        let mut report = work(l);
        report.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        report
    })
}

/// `diff`: run the reference and candidate implementations and compare them.
pub fn diff(
    loaded: &[Loaded],
    reference: &Implementation,
    candidate: &Implementation,
    allow: &[Divergence],
    norm: &dyn Normaliser,
    out: Option<&Path>,
    jobs: usize,
) -> Vec<FileReport> {
    per_file(loaded, jobs, |l| {
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
}

/// `record`: run the reference implementation and write goldens to `golden_dir`.
pub fn record(
    loaded: &[Loaded],
    reference: &Implementation,
    golden_dir: &Path,
    norm: &dyn Normaliser,
    out: Option<&Path>,
    jobs: usize,
) -> Vec<FileReport> {
    per_file(loaded, jobs, |l| {
        let s = match &l.scenario {
            Ok(s) => s,
            Err(e) => {
                return FileReport::failed(&l.file, None, format!("invalid scenario: {e}"));
            }
        };
        let result = run_scenario(s, reference).and_then(|r| {
            write_log(out, &r)?;
            // A run that fails a step is not a reference: it judges the scenario as failed
            // and leaves the golden alone, so a broken recording never becomes the baseline.
            if r.failed() {
                return Ok(r);
            }
            std::fs::create_dir_all(golden_dir)
                .map_err(|e| format!("create {}: {e}", golden_dir.display()))?;
            let mut text = serde_json::to_string_pretty(&golden_json(&s.id, &observe(&r, norm)))
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
}

/// `check`: run the candidate implementation and compare it with the goldens.
pub fn check(
    loaded: &[Loaded],
    candidate: &Implementation,
    golden_dir: &Path,
    allow: &[Divergence],
    norm: &dyn Normaliser,
    out: Option<&Path>,
    jobs: usize,
) -> Vec<FileReport> {
    per_file(loaded, jobs, |l| {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::Verdict;
    use crate::normalise::FullNormaliser;

    fn scratch(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("wg-modes-{name}-{}-{nanos}", std::process::id()))
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A scenario whose first step is `git --version`, followed by `after`.
    fn scenario(id: &str, after: &str) -> String {
        format!(r#"{{"id":"{id}","covers":[],"steps":[{{"cli":["--version"]}},{after}]}}"#)
    }

    #[test]
    fn load_descends_into_area_directories() {
        let dir = scratch("load");
        let pass = r#"{"assert":{"exit":0}}"#;
        write(&dir.join("top.json"), &scenario("top", pass));
        write(&dir.join("git/one.json"), &scenario("one", pass));
        write(&dir.join("runs/deep/two.json"), &scenario("two", pass));
        write(&dir.join("runs/deep/notes.md"), "not a scenario");
        write(&dir.join("git/wrong.json"), &scenario("other", pass));

        let loaded = load_scenarios(&dir).unwrap();
        let files: Vec<&str> = loaded.iter().map(|l| l.file.as_str()).collect();
        assert_eq!(
            files,
            [
                "git/one.json",
                "git/wrong.json",
                "runs/deep/two.json",
                "top.json"
            ]
        );
        assert!(loaded[0].scenario.is_ok());
        let wrong = loaded[1].scenario.as_ref().unwrap_err();
        assert!(
            wrong.contains("must equal the file name `wrong`"),
            "{wrong}"
        );
        assert!(loaded[2].scenario.is_ok() && loaded[3].scenario.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_of_one_file_keeps_its_plain_name() {
        let dir = scratch("single");
        let path = dir.join("git/only.json");
        write(&path, &scenario("only", r#"{"assert":{"exit":0}}"#));
        let loaded = load_scenarios(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].file, "only.json");
        assert!(loaded[0].scenario.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn results_come_back_in_item_order_whatever_order_workers_finish_in() {
        let items: Vec<u64> = (0..6).collect();
        let finished = Mutex::new(Vec::new());
        // Earlier items sleep longer, so with three workers they finish out of order.
        let out = run_in_order(&items, 3, |&i| {
            std::thread::sleep(std::time::Duration::from_millis(10 * (6 - i)));
            finished.lock().unwrap().push(i);
            i * 10
        });
        let expected: Vec<u64> = items.iter().map(|i| i * 10).collect();
        assert_eq!(out, expected);
        assert_ne!(
            finished.into_inner().unwrap(),
            items,
            "the workers must actually finish out of order"
        );
        for jobs in [1, 2, 6, 64] {
            assert_eq!(run_in_order(&items, jobs, |&i| i * 10), expected);
        }
        let none: Vec<u64> = Vec::new();
        assert!(run_in_order(&none, 4, |&i| i).is_empty());
    }

    #[test]
    fn record_writes_a_golden_only_when_the_run_passes() {
        let dir = scratch("record");
        write(
            &dir.join("area/passes.json"),
            &scenario("passes", r#"{"assert":{"exit":0}}"#),
        );
        write(
            &dir.join("area/fails.json"),
            &scenario("fails", r#"{"assert":{"exit":7}}"#),
        );
        let golden = dir.join("golden");
        let loaded = load_scenarios(&dir).unwrap();
        let reference = Implementation::from_command("ref", "git", Vec::new()).unwrap();

        let reports = record(
            &loaded,
            &reference,
            &golden,
            &FullNormaliser::default(),
            None,
            2,
        );

        let by_file = |name: &str| reports.iter().find(|r| r.file == name).unwrap();
        let passed = by_file("area/passes.json");
        assert_eq!(passed.verdict, Verdict::Equal);
        assert!(golden.join("passes.json").exists());
        let failed = by_file("area/fails.json");
        assert_eq!(failed.verdict, Verdict::Failed);
        assert!(
            failed.problems.iter().any(|p| p.contains("step 1")),
            "{:?}",
            failed.problems
        );
        assert!(!golden.join("fails.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
