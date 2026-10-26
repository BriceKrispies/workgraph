//! `claude hook`: the fast path of the `workgraph` binary, dispatched by [`crate::run`] before the clap
//! tree is built (P01, O15). It reads one hook payload from stdin, ingests it, spools it when ingestion
//! fails, and always exits 0 with nothing on stdout (`integrations/claude_code/hook_cli.py::run_hook`).

use std::io::{Read, Write};
use std::path::Path;

use wg_runtime::{draw, resolve_paths, spool_record};
use wg_telemetry::ingest::HookEnv;
use wg_telemetry::spool::{MAX_INPUT_BYTES, hook_record, payload_from_input};
use wg_telemetry_feature::service::hooks::ingest_direct;

/// Runs the hook for the arguments after `claude hook` and returns the exit code, which is always 0.
#[must_use]
pub fn run(args: &[String]) -> u8 {
    let debug = args.iter().any(|arg| arg == "--debug");
    let Ok((received, _)) = draw() else {
        return 0;
    };
    let received_at = received.format_isoformat();
    let mut input = Vec::new();
    let limit = u64::try_from(MAX_INPUT_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    if std::io::stdin()
        .lock()
        .take(limit)
        .read_to_end(&mut input)
        .is_err()
    {
        return 0;
    }
    let env = HookEnv {
        attempt_id: std::env::var("WORKGRAPH_ATTEMPT_ID").ok(),
        task_id: std::env::var("WORKGRAPH_TASK_ID").ok(),
        workspace: std::env::var("WORKGRAPH_WORKSPACE").ok(),
    };
    let mut stderr = std::io::stderr().lock();
    process(None, &received_at, &env, &input, debug, &mut stderr)
}

/// The hook after its stdin read, against the home `home` names (the default home when `None`): the
/// spool directory is created, the record is ingested directly, and when that fails the redacted record
/// is spooled for replay. `--debug` reports a spooled failure on stderr. Always returns 0.
pub fn process(
    home: Option<&Path>,
    received_at: &str,
    env: &HookEnv,
    input: &[u8],
    debug: bool,
    stderr: &mut dyn Write,
) -> u8 {
    let Ok(paths) = resolve_paths(home) else {
        return 0;
    };
    if std::fs::create_dir_all(paths.spool_dir().as_str()).is_err() {
        return 0;
    }
    let record = hook_record(received_at, env, payload_from_input(input));
    if let Err(failure) = ingest_direct(home, &record) {
        let _ = spool_record(home, &record, &failure);
        if debug {
            let _ = writeln!(stderr, "workgraph hook spooled: {}", failure.message);
        }
    }
    0
}
