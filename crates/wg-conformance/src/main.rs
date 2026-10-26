//! Command line for the conformance runner.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use wg_conformance::compare::{FileReport, parse_divergences, report_json, report_text, tally};
use wg_conformance::dbdump::canonical_dump;
use wg_conformance::modes::{check, diff, load_scenarios, record};
use wg_conformance::normalise::FullNormaliser;
use wg_conformance::replay;
use wg_conformance::runner::Implementation;

const USAGE: &str = "usage: wg-conformance <validate|diff|record|check> [options]
       wg-conformance dbdump DATABASE [-o FILE]   canonical database dump (as dbdump.py dump)
       wg-conformance replay-http (--rust COMMAND | --python COMMAND) [--contracts DIR] [--file NAME]...
                            replay the CF-04 HTTP recordings (default contracts/http)
  --scenarios PATH     scenario directory or file (default tests/conformance/scenarios)
  --python COMMAND     reference implementation command line (diff, record)
  --rust COMMAND       candidate implementation command line (diff, check)
  --golden DIR         golden directory (default tests/conformance/golden)
  --divergences FILE   allowlist (default tests/conformance/divergences.toml, optional)
  --out DIR            write step logs under DIR/<implementation>/
  --report FILE        write the JSON report
  --env KEY=VALUE      extra environment for both implementations (repeatable)";

struct Options {
    mode: String,
    scenarios: PathBuf,
    python: Option<String>,
    rust: Option<String>,
    golden: PathBuf,
    divergences: PathBuf,
    out: Option<PathBuf>,
    report: Option<PathBuf>,
    env: Vec<(String, String)>,
}

fn parse_args(args: Vec<String>) -> Result<Options, String> {
    let mut it = args.into_iter();
    let mode = it.next().ok_or("missing mode")?;
    let mut o = Options {
        mode,
        scenarios: "tests/conformance/scenarios".into(),
        python: None,
        rust: None,
        golden: "tests/conformance/golden".into(),
        divergences: "tests/conformance/divergences.toml".into(),
        out: None,
        report: None,
        env: Vec::new(),
    };
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--scenarios" => o.scenarios = value()?.into(),
            "--python" => o.python = Some(value()?),
            "--rust" => o.rust = Some(value()?),
            "--golden" => o.golden = value()?.into(),
            "--divergences" => o.divergences = value()?.into(),
            "--out" => o.out = Some(value()?.into()),
            "--report" => o.report = Some(value()?.into()),
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

fn run(o: &Options) -> Result<Vec<FileReport>, String> {
    let loaded = load_scenarios(&o.scenarios)?;
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
                Ok(s) => FileReport {
                    file: l.file.clone(),
                    scenario: Some(s.id.clone()),
                    verdict: wg_conformance::compare::Verdict::Equal,
                    diffs: Vec::new(),
                    problems: Vec::new(),
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
        ),
        "record" => record(
            &loaded,
            &implementation("python", &o.python, o)?,
            &o.golden,
            &norm,
            out,
        ),
        "check" => check(
            &loaded,
            &implementation("rust", &o.rust, o)?,
            &o.golden,
            &allow,
            &norm,
            out,
        ),
        other => return Err(format!("unknown mode `{other}`")),
    })
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
    match run(&options) {
        Ok(reports) => {
            print!("{}", report_text(&options.mode, &reports));
            if let Some(path) = &options.report {
                let mut text = serde_json::to_string_pretty(&report_json(&options.mode, &reports))
                    .unwrap_or_default();
                text.push('\n');
                if let Err(e) = std::fs::write(path, text) {
                    eprintln!("error: write {}: {e}", path.display());
                    return ExitCode::from(2);
                }
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
