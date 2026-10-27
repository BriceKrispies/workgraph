//! The stack executor against the Python one (SH-25): the `stack_integrate` and `stack_verify`
//! handlers, the rebuild, the landing and the manual ejection, run on real git repositories.
//!
//! Each scenario is seeded once with Python (`tools/stack_parity/parity.py`: a home, a repository,
//! a stack whose children have run their agents and whose integration jobs are still queued). The
//! seed is restored and run twice, once by the Python executor and once by this crate's, and the
//! normalised dumps of the rows, events, files and git refs must be equal.
//!
//! The scenarios need `uv` and the Python package; without `uv` they report that and pass.

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use wg_runtime::{HandlerRegistry, Queue, Worker, WorkerOptions, human_context};

type TestResult = Result<(), Box<dyn Error>>;

/// One scenario at a time: each starts several Python processes and a worker.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const STACK_KINDS: [&str; 2] = ["stack_integrate", "stack_verify"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn have_uv() -> bool {
    Command::new("uv")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Runs `parity.py` with `args`; its standard output on success.
fn parity(args: &[&str]) -> Result<String, Box<dyn Error>> {
    let out = Command::new("uv")
        .args(["run", "--quiet", "python", "tools/stack_parity/parity.py"])
        .args(args)
        .current_dir(root())
        .env("PYTHONIOENCODING", "utf-8")
        .output()?;
    if !out.status.success() {
        return Err(format!(
            "parity.py {} failed:\n{}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Drains the stack jobs of the home with a worker.
fn drain(home: &Path) -> TestResult {
    let mut worker = Worker::open(
        WorkerOptions {
            home: Some(home.to_path_buf()),
            concurrency: Some(2),
            kinds: STACK_KINDS.iter().map(|kind| (*kind).to_owned()).collect(),
            poll_interval: Some(Duration::from_millis(50)),
            maintenance_interval: Some(Duration::from_millis(200)),
            ..WorkerOptions::default()
        },
        HandlerRegistry::from_features(&[&wg_stacks_feature::FEATURE]),
    )?;
    worker.run_until_idle(Duration::from_secs(180))?;
    Ok(())
}

/// The Rust executor's run of the scenario's flow.
fn run_rust(home: &Path, flow: &[Value]) -> TestResult {
    for step in flow {
        if step.get("drain").and_then(Value::as_bool) == Some(true) {
            drain(home)?;
        }
        if let Some(index) = step.get("eject").and_then(Value::as_i64) {
            let reason = step
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut queue = Queue::open(Some(home), "test")?;
            let context = human_context(Some("tester"))?;
            wg_stacks_feature::service::exec::eject(
                &mut queue.app,
                &context,
                Some("main"),
                "WG-1",
                &format!("WG-{}", index + 2),
                reason,
            )?;
        }
    }
    Ok(())
}

fn scenario(name: &str) -> TestResult {
    if !have_uv() {
        eprintln!("uv is not installed: skipping the {name} parity scenario");
        return Ok(());
    }
    let _guard = ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("sh25")
        .join(name);
    let live = base.join("live");
    let seed = base.join("seed");
    // SH25_REUSE_SEED keeps an earlier seed, which saves the slowest step when iterating.
    let reuse = std::env::var_os("SH25_REUSE_SEED").is_some() && seed.exists();
    if !reuse {
        if base.exists() {
            std::fs::remove_dir_all(&base)?;
        }
        std::fs::create_dir_all(&base)?;
        parity(&["seed", &text(&live), &text(&seed), name])?;
    } else {
        parity(&["restore", &text(&seed), &text(&live)])?;
    }

    // Python's run.
    parity(&["run", &text(&live), name])?;
    let python = base.join("python.json");
    parity(&["dump", &text(&live), &text(&python)])?;

    // Rust's run on the same seed.
    parity(&["restore", &text(&seed), &text(&live)])?;
    let flow: Value = serde_json::from_str(&parity(&["flow", name])?)?;
    let steps = flow
        .get("flow")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    run_rust(&live.join("home"), &steps)?;
    let rust = base.join("rust.json");
    parity(&["dump", &text(&live), &text(&rust)])?;

    parity(&["compare", &text(&python), &text(&rust)]).map(|_| ())
}

#[test]
fn verify_pass() -> TestResult {
    scenario("verify_pass")
}

#[test]
fn land() -> TestResult {
    scenario("land")
}

#[test]
fn no_full_checks() -> TestResult {
    scenario("no_full_checks")
}

#[test]
fn bisect() -> TestResult {
    scenario("bisect")
}

#[test]
fn regression() -> TestResult {
    scenario("regression")
}

#[test]
fn tolerated() -> TestResult {
    scenario("tolerated")
}

#[test]
fn base_fails() -> TestResult {
    scenario("base_fails")
}

#[test]
fn errored() -> TestResult {
    scenario("errored")
}

#[test]
fn conflict() -> TestResult {
    scenario("conflict")
}

#[test]
fn dependent() -> TestResult {
    scenario("dependent")
}

#[test]
fn manual_eject() -> TestResult {
    scenario("manual_eject")
}

#[test]
fn stale() -> TestResult {
    scenario("stale")
}

#[test]
fn stale_land() -> TestResult {
    scenario("stale_land")
}
