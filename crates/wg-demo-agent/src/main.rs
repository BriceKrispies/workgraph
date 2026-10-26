//! wg-demo-agent: the scripted workgraph.agent.v1 demo agent and the demo checks.
//!
//! The Rust counterpart of `workgraph/demo/scripted_agent.py` and of the two check programs the
//! demo configures (DV4). Three entry points:
//!
//! * `workgraph-demo-agent [--mode M] [--file F] <manifest>`: the agent; the request is read from
//!   stdin and protocol messages are written to stdout;
//! * `workgraph-demo-agent check`: the per-repository contract check, run in the checkout;
//! * `workgraph-demo-agent integration`: the cross-repository check, configured through the
//!   `WORKGRAPH_REPO_<NAME>_PATH` environment variables.

mod agent;
mod checks;

use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("check") => checks::contract_check(Path::new("."), &mut std::io::stdout()),
        Some("integration") => {
            checks::integration(|name| std::env::var(name).ok(), &mut std::io::stdout())
        }
        _ => run_agent(&args),
    };
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn run_agent(args: &[String]) -> i32 {
    let parsed = match wg_demoproto::AgentArgs::parse(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("workgraph-demo-agent: {message}");
            return 2;
        }
    };
    let mut request = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut request) {
        eprintln!("workgraph-demo-agent: cannot read the request: {error}");
        return 1;
    }
    let mut out = std::io::stdout().lock();
    let code = match agent::run(&parsed, &request, &mut out) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("workgraph-demo-agent: {error}");
            1
        }
    };
    let _ = out.flush();
    code
}
