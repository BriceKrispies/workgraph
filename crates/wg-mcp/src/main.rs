//! `wg-mcp`: the MCP server over stdio as a program, for the conformance replay and for hosts that
//! run it apart from the `workgraph` binary. Flags as `workgraph mcp`: `--agent-name`, `--grant`
//! (repeatable); a leading `mcp` word is skipped. The HTTP transport is served only by
//! `workgraph mcp --transport http`, behind the API token (WG-3764).

use std::process::ExitCode;

use wg_mcp::{AppBackend, serve_stdio};
use wg_mcpspec::options::{ServeOptions, Transport};
use wg_types::{ErrorCode, WgError};

/// The features this program serves: the decisions and automation features, as the `workgraph`
/// binary lists them.
static FEATURES: &[&wg_runtime::Feature] =
    &[&wg_decisions::FEATURE, &wg_automation_feature::FEATURE];

const HTTP_REFUSED: &str = "the standalone wg-mcp serves stdio only; \
`workgraph mcp --transport http` serves the endpoint behind the API token";

fn run(args: &[String]) -> Result<(), WgError> {
    let options = ServeOptions::parse(args)?;
    if options.transport == Transport::Http {
        return Err(WgError::new(ErrorCode::WorkgraphError, HTTP_REFUSED));
    }
    let app = wg_runtime::App::open(None)?;
    let backend = Box::new(AppBackend {
        app,
        features: FEATURES,
    });
    let agent = options.agent();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| WgError::new(ErrorCode::WorkgraphError, err.to_string()))?;
    runtime.block_on(serve_stdio(agent, backend))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {}", err.message);
            ExitCode::from(err.exit_code())
        }
    }
}
