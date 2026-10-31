//! Command line for the conformance runner.

use serde_json::json;
use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use wg_conformance::budget::{self, Budget};
use wg_conformance::compare::{FileReport, parse_divergences, report_json, report_text, tally};
use wg_conformance::coverage::{coverage, inventory_ids, marked_tests, parse_exclusions};
use wg_conformance::dbdump::canonical_dump;
use wg_conformance::modes::{Loaded, check, diff, load_scenarios, record};
use wg_conformance::normalise::FullNormaliser;
use wg_conformance::replay;
use wg_conformance::runner::Implementation;
use wg_conformance::wait;

const USAGE: &str = "usage: wg-conformance <validate|diff|record|check|coverage> [options]
       wg-conformance dbdump DATABASE [-o FILE]   canonical database dump (as dbdump.py dump)
       wg-conformance mcp-replay --command COMMAND [--transcript FILE]   replay the CF-03 stdio transcript
       wg-conformance replay-http (--rust COMMAND | --python COMMAND) [--contracts DIR] [--file NAME]...
                            replay the CF-04 HTTP recordings (default contracts/http)
  --scenarios PATH     scenario directory or file (default tests/conformance/scenarios)
  --contracts FILE     contract inventory for coverage (default docs/rust-rewrite-contracts.md)
  --allowlist FILE     coverage exclusions with reasons (default tests/conformance/coverage-allowlist.toml, optional)
  --python COMMAND     reference implementation command line (diff, record); quote a path with spaces
  --rust COMMAND       candidate implementation command line (diff, check)
  --golden DIR         golden directory (default tests/conformance/golden)
  --divergences FILE   allowlist (default tests/conformance/divergences.toml, optional)
  --out DIR            write step logs under DIR/<implementation>/
  --report FILE        write the JSON report (per-file duration_ms and slow_steps)
  --env KEY=VALUE      extra environment for both implementations (repeatable)
  --jobs N             scenario files run concurrently in diff, record and check
                       (default: the available parallelism; the report is the same for any N)
  --only IDS           run only these scenario ids, comma separated (repeatable; unknown ids are an error)
  --list FILE          run only the scenario ids listed in FILE, one per line (# comments allowed)
  --stall-timeout SECS a process, MCP reply or HTTP response that makes no progress for SECS seconds
                       is killed and reported as a step problem (default 10)
  --budget SECS        hard wall-clock budget for the whole run (validate, diff, record, check;
                       default 120, above 120 is refused). When it is spent the run starts no more
                       scenarios, kills the ones running, fails the unfinished ones and exits 1";

struct Options {
    mode: String,
    scenarios: PathBuf,
    contracts: PathBuf,
    allowlist: PathBuf,
    python: Option<String>,
    rust: Option<String>,
    golden: PathBuf,
    divergences: PathBuf,
    out: Option<PathBuf>,
    report: Option<PathBuf>,
    env: Vec<(String, String)>,
    jobs: usize,
    /// Scenario ids named by `--only` and `--list`; empty runs every scenario.
    only: BTreeSet<String>,
    stall: Option<std::time::Duration>,
    budget: Duration,
}

/// Read a `--list` file: one scenario id per line, blank lines and `#` comments skipped.
fn read_id_list(path: &str) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    Ok(text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

/// Keep the scenarios whose id (their file name without `.json`) is in `wanted`. Every wanted id
/// must match a scenario, so a misspelt id fails instead of running nothing.
fn select(loaded: Vec<Loaded>, wanted: &BTreeSet<String>) -> Result<Vec<Loaded>, String> {
    if wanted.is_empty() {
        return Ok(loaded);
    }
    let stem = |file: &str| {
        let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
        name.strip_suffix(".json").unwrap_or(name).to_owned()
    };
    let known: BTreeSet<String> = loaded.iter().map(|l| stem(&l.file)).collect();
    let missing: Vec<&str> = wanted
        .iter()
        .filter(|id| !known.contains(id.as_str()))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(format!("no scenario with id {}", missing.join(", ")));
    }
    Ok(loaded
        .into_iter()
        .filter(|l| wanted.contains(&stem(&l.file)))
        .collect())
}

fn parse_args(args: Vec<String>) -> Result<Options, String> {
    let mut it = args.into_iter();
    let mode = it.next().ok_or("missing mode")?;
    let mut o = Options {
        mode,
        scenarios: "tests/conformance/scenarios".into(),
        contracts: "docs/rust-rewrite-contracts.md".into(),
        allowlist: "tests/conformance/coverage-allowlist.toml".into(),
        python: None,
        rust: None,
        golden: "tests/conformance/golden".into(),
        divergences: "tests/conformance/divergences.toml".into(),
        out: None,
        report: None,
        env: Vec::new(),
        jobs: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        only: BTreeSet::new(),
        stall: None,
        budget: budget::CEILING,
    };
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--only" => o.only.extend(
                value()?
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            ),
            "--list" => o.only.extend(read_id_list(&value()?)?),
            "--stall-timeout" => o.stall = Some(wait::parse_stall_timeout(&value()?)?),
            "--budget" => o.budget = budget::parse(&value()?)?,
            "--scenarios" => o.scenarios = value()?.into(),
            "--contracts" => o.contracts = value()?.into(),
            "--allowlist" => o.allowlist = value()?.into(),
            "--python" => o.python = Some(value()?),
            "--rust" => o.rust = Some(value()?),
            "--golden" => o.golden = value()?.into(),
            "--divergences" => o.divergences = value()?.into(),
            "--out" => o.out = Some(value()?.into()),
            "--report" => o.report = Some(value()?.into()),
            "--jobs" => {
                let text = value()?;
                o.jobs = match text.parse::<usize>() {
                    Ok(n) if n >= 1 => n,
                    _ => return Err(format!("--jobs needs a positive number, not `{text}`")),
                };
            }
            "--env" => {
                let kv = value()?;
                let (k, v) = kv.split_once('=').ok_or("--env needs KEY=VALUE")?;
                o.env.push((k.to_owned(), v.to_owned()));
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(o)
}

fn implementation(name: &str, cmd: &Option<String>, o: &Options) -> Result<Implementation, String> {
    let cmd = cmd
        .as_deref()
        .ok_or_else(|| format!("this mode needs --{name}"))?;
    Implementation::from_command(name, cmd, o.env.clone())
}

fn run(o: &Options, budget: &Budget) -> Result<Vec<FileReport>, String> {
    let loaded = select(load_scenarios(&o.scenarios)?, &o.only)?;
    let allow = match std::fs::read_to_string(&o.divergences) {
        Ok(t) => parse_divergences(&t)?,
        Err(_) => Vec::new(),
    };
    let norm = FullNormaliser::default();
    let out = o.out.as_deref();
    Ok(match o.mode.as_str() {
        "validate" => loaded
            .iter()
            .map(|l| match &l.scenario {
                _ if budget.expired() => FileReport::failed(
                    &l.file,
                    l.scenario.as_ref().ok().map(|s| s.id.clone()),
                    budget.problem(),
                ),
                Ok(s) => FileReport {
                    file: l.file.clone(),
                    scenario: Some(s.id.clone()),
                    verdict: wg_conformance::compare::Verdict::Equal,
                    diffs: Vec::new(),
                    problems: Vec::new(),
                    duration_ms: 0,
                    slow_steps: Vec::new(),
                },
                Err(e) => FileReport::failed(&l.file, None, format!("invalid scenario: {e}")),
            })
            .collect(),
        "diff" => diff(
            &loaded,
            &implementation("python", &o.python, o)?,
            &implementation("rust", &o.rust, o)?,
            &allow,
            &norm,
            out,
            o.jobs,
            budget,
        ),
        "record" => record(
            &loaded,
            &implementation("python", &o.python, o)?,
            &o.golden,
            &norm,
            out,
            o.jobs,
            budget,
        ),
        "check" => check(
            &loaded,
            &implementation("rust", &o.rust, o)?,
            &o.golden,
            &allow,
            &norm,
            out,
            o.jobs,
            budget,
        ),
        other => return Err(format!("unknown mode `{other}`")),
    })
}

/// `coverage`: which contract ids of the inventory the scenarios and tests cover, or exclude with
/// a reason (CR-07). Returns whether the coverage sources are clean: every scenario loads, every
/// marker names contract ids, and every name is in the inventory.
fn coverage_mode(o: &Options) -> Result<bool, String> {
    let markdown = std::fs::read_to_string(&o.contracts)
        .map_err(|e| format!("read {}: {e}", o.contracts.display()))?;
    let inventory = inventory_ids(&markdown);
    let loaded = if o.scenarios.exists() {
        load_scenarios(&o.scenarios)?
    } else {
        Vec::new()
    };
    let mut sources = Vec::new();
    let mut invalid = Vec::new();
    for item in &loaded {
        match &item.scenario {
            Ok(s) => sources.push((s.id.clone(), s.covers.clone())),
            Err(e) => invalid.push(format!("{}: {e}", item.file)),
        }
    }
    for file in marker_files()? {
        let label = file.to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&file).map_err(|e| format!("read {label}: {e}"))?;
        let (tests, problems) = marked_tests(&label, &text);
        sources.extend(tests);
        invalid.extend(problems);
    }
    let exclusions = match std::fs::read_to_string(&o.allowlist) {
        Ok(t) => parse_exclusions(&t)?,
        Err(_) => Vec::new(),
    };
    let joined = coverage(&inventory, &sources, &exclusions);
    print!(
        "{}",
        wg_conformance::coverage::report_text(&joined, &invalid)
    );
    if let Some(path) = &o.report {
        let mut text =
            serde_json::to_string_pretty(&wg_conformance::coverage::report_json(&joined, &invalid))
                .unwrap_or_default();
        text.push('\n');
        std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(invalid.is_empty() && joined.unknown.is_empty() && joined.unknown_exclusions.is_empty())
}

/// The files that may carry `covers` markers: the Rust sources and tests of every crate
/// (`crates/*/src`, `crates/*/tests`) and the Python tests under `tests/`, relative to the working
/// directory like the other paths. Sorted, so the report is stable.
fn marker_files() -> Result<Vec<PathBuf>, String> {
    let mut roots: Vec<(PathBuf, &str)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir("crates") {
        for entry in entries.flatten() {
            for sub in ["src", "tests"] {
                let dir = entry.path().join(sub);
                if dir.is_dir() {
                    roots.push((dir, "rs"));
                }
            }
        }
    }
    roots.push((PathBuf::from("tests"), "py"));
    let mut files = Vec::new();
    for (root, ext) in &roots {
        if root.is_dir() {
            collect_files(root, ext, &mut files)?;
        }
    }
    files.sort();
    Ok(files)
}

fn collect_files(dir: &std::path::Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|e| format!("read {}: {e}", dir.display()))?
            .path();
        if path.is_dir() {
            collect_files(&path, ext, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(path);
        }
    }
    Ok(())
}

/// `dbdump DATABASE [-o FILE]`: the same output as `tools/contracts/dbdump.py dump`.
fn dbdump(args: &[String]) -> ExitCode {
    let mut database = None;
    let mut output = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" | "--output" => match it.next() {
                Some(path) => output = Some(PathBuf::from(path)),
                None => {
                    eprintln!("error: {arg} needs a value\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            other if database.is_none() && !other.starts_with('-') => {
                database = Some(PathBuf::from(other));
            }
            other => {
                eprintln!("error: unknown option `{other}`\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(database) = database else {
        eprintln!("error: dbdump needs a DATABASE\n{USAGE}");
        return ExitCode::from(2);
    };
    let bytes = match canonical_dump(&database) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("error: {}: {e}", database.display());
            return ExitCode::from(1);
        }
    };
    let written = match output {
        Some(path) => {
            std::fs::write(&path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))
        }
        None => std::io::stdout()
            .write_all(&bytes)
            .map_err(|e| format!("write stdout: {e}")),
    };
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

/// `replay-http`: the CF-04 replay of `contracts/http/*.json` (see `wg_conformance::replay`).
fn replay_http(args: &[String]) -> ExitCode {
    let mut command = None;
    let mut name = "rust";
    let mut contracts = PathBuf::from("contracts/http");
    let mut files: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned();
        let parsed = match arg.as_str() {
            "--rust" => value().map(|v| {
                name = "rust";
                command = Some(v);
            }),
            "--python" => value().map(|v| {
                name = "python";
                command = Some(v);
            }),
            "--contracts" => value().map(|v| contracts = PathBuf::from(v)),
            "--file" => value().map(|v| files.push(v)),
            _ => None,
        };
        if parsed.is_none() {
            eprintln!(
                "error: bad or incomplete option `{arg}`
{USAGE}"
            );
            return ExitCode::from(2);
        }
    }
    let Some(command) = command else {
        eprintln!(
            "error: replay-http needs --rust or --python
{USAGE}"
        );
        return ExitCode::from(2);
    };
    let imp = match Implementation::from_command(name, &command, Vec::new()) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if files.is_empty() {
        files = replay::FILES.iter().map(|f| (*f).to_owned()).collect();
    }
    let mut reports = Vec::new();
    for file in &files {
        match replay::replay_file(&imp, &contracts, file) {
            Ok(r) => reports.push(r),
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        }
    }
    print!("{}", replay::report_text(&reports));
    if replay::failed(&reports) == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("dbdump") {
        return dbdump(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("mcp-replay") {
        return wg_conformance::mcp::replay_command(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("replay-http") {
        return replay_http(&args[1..]);
    }
    let options = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if let Some(window) = options.stall {
        wait::set_stall_timeout(window);
    }
    if options.mode == "coverage" {
        return match coverage_mode(&options) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(1),
            Err(e) => {
                eprintln!(
                    "error: {e}
{USAGE}"
                );
                ExitCode::from(2)
            }
        };
    }
    let budget = Budget::start(options.budget);
    budget::arm(budget);
    budget::watchdog(budget);
    match run(&options, &budget) {
        Ok(reports) => {
            let elapsed = budget.elapsed();
            let exceeded = budget.expired();
            print!("{}", report_text(&options.mode, &reports));
            if let Some(path) = &options.report {
                let mut json = report_json(&options.mode, &reports);
                json["budget_exceeded"] = json!(exceeded);
                json["elapsed_ms"] = json!(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX));
                let mut text = serde_json::to_string_pretty(&json).unwrap_or_default();
                text.push('\n');
                if let Err(e) = std::fs::write(path, text) {
                    eprintln!("error: write {}: {e}", path.display());
                    return ExitCode::from(2);
                }
            }
            if exceeded {
                let problem = budget.problem();
                let cut = reports
                    .iter()
                    .filter(|r| r.problems.first() == Some(&problem))
                    .count();
                println!(
                    "{problem}: ran {:.1} s; {cut} of {} file(s) cut",
                    elapsed.as_secs_f64(),
                    reports.len()
                );
                return ExitCode::from(1);
            }
            if tally(&reports).2 == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("error: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(file: &str) -> Loaded {
        Loaded {
            file: file.to_owned(),
            scenario: Err("not parsed in this test".to_owned()),
        }
    }

    #[test]
    fn only_keeps_the_named_scenarios_and_rejects_unknown_ids() {
        let all = || {
            vec![
                loaded("git/one.json"),
                loaded("two.json"),
                loaded("runs/three.json"),
            ]
        };
        let wanted: BTreeSet<String> = ["two", "three"].map(str::to_owned).into();
        let kept = select(all(), &wanted).unwrap();
        let files: Vec<&str> = kept.iter().map(|l| l.file.as_str()).collect();
        assert_eq!(files, ["two.json", "runs/three.json"]);
        assert!(select(all(), &BTreeSet::new()).unwrap().len() == 3);
        let typo: BTreeSet<String> = ["two", "tow"].map(str::to_owned).into();
        let Err(err) = select(all(), &typo) else {
            panic!("unknown id accepted")
        };
        assert!(err.contains("tow") && !err.contains("two,"), "{err}");
    }

    #[test]
    fn list_files_skip_blank_lines_and_comments() {
        let dir = std::env::temp_dir().join(format!("wg-conf-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fast.txt");
        std::fs::write(&path, "# routine subset\none\n\n  two  \n").unwrap();
        let ids = read_id_list(path.to_str().unwrap()).unwrap();
        assert_eq!(ids, ["one", "two"]);
        assert!(read_id_list(dir.join("absent.txt").to_str().unwrap()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
