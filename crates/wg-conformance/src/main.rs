//! Command line for the conformance runner.

use std::path::PathBuf;
use std::process::ExitCode;
use wg_conformance::compare::{
    FileReport, RootsNormaliser, parse_divergences, report_json, report_text, tally,
};
use wg_conformance::modes::{check, diff, load_scenarios, record};
use wg_conformance::runner::Implementation;

const USAGE: &str = "usage: wg-conformance <validate|diff|record|check> [options]
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
    let norm = RootsNormaliser;
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

fn main() -> ExitCode {
    let options = match parse_args(std::env::args().skip(1).collect()) {
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
