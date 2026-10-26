//! `wg-mcp`: the MCP server as a program, for the conformance replay and for hosts that run it
//! apart from the `workgraph` binary. Flags as `workgraph mcp`: `--transport stdio|http`,
//! `--agent-name`, `--grant` (repeatable), `--host`, `--port`; a leading `mcp` word is skipped.

use std::process::ExitCode;

use tokio_util::sync::CancellationToken;
use wg_mcp::{AppBackend, serve_http, serve_stdio};
use wg_mcpspec::options::{ServeOptions, Transport};
use wg_types::WgError;

fn run(args: &[String]) -> Result<(), WgError> {
    let options = ServeOptions::parse(args)?;
    let app = wg_runtime::App::open(None)?;
    let backend = Box::new(AppBackend(app));
    let agent = options.agent();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| WgError::new(wg_types::ErrorCode::WorkgraphError, err.to_string()))?;
    runtime.block_on(async {
        match options.transport {
            Transport::Stdio => serve_stdio(agent, backend).await,
            Transport::Http => {
                let listener = std::net::TcpListener::bind((options.host.as_str(), options.port))
                    .map_err(|err| {
                    WgError::new(wg_types::ErrorCode::WorkgraphError, err.to_string())
                })?;
                eprintln!(
                    "MCP Streamable HTTP on http://{}:{}/mcp/",
                    options.host, options.port
                );
                serve_http(listener, agent, backend, CancellationToken::new()).await
            }
        }
    })
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
