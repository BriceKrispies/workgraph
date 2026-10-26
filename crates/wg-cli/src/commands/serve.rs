//! `workgraph serve` and `workgraph mcp` (IF-23, `cli/commands/servers.py`).
//!
//! `serve` runs the HTTP API and web UI of `wg-server`, optionally with the job worker of
//! `wg-runtime` on a thread and the `/mcp/` mount of `wg-mcp`. `mcp` runs the MCP server over
//! stdio, or over Streamable HTTP behind the same server and token as `serve`. The decisions
//! (the `server` settings, the LAN plan, the refusals, the start-up lines) are `wg-serverspec`'s
//! and `wg-mcpspec`'s; this module holds the sockets, the threads and the signal.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use clap::ArgMatches;
use tokio_util::sync::CancellationToken;
use wg_clispec::CliFailure;
use wg_clispec::args::{option_flag, option_text};
use wg_clispec::servers::{self, mcp_arguments, pair, port_option};
use wg_mcp::{Agent, AppBackend, http_router, serve_stdio};
use wg_mcpspec::options::{ServeOptions, Transport};
use wg_mcpspec::permissions::agent_permissions;
use wg_runtime::{App, HandlerRegistry, RunOptions, Worker, WorkerOptions};
use wg_server::{LanOptions, Server, ServerOptions, Ui, bind_listeners, groups, host_name};
use wg_serverspec::lan::{
    LOOPBACK_ONLY_NOTICE, LanPlan, lan_plan, listening_notice, local_network_markup,
};
use wg_serverspec::serve::{Settings, bind_refusal};

use super::{FEATURES, Group};
use crate::{CliState, console};

/// The `serve` command.
pub const SERVE: Group = Group {
    name: "serve",
    command: servers::serve_command,
    run: run_serve,
};

/// The `mcp` command.
pub const MCP: Group = Group {
    name: "mcp",
    command: servers::mcp_command,
    run: run_mcp,
};

fn failed(message: impl Into<String>) -> CliFailure {
    CliFailure::fail(message, 2)
}

fn runtime() -> Result<tokio::runtime::Runtime, CliFailure> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| CliFailure::fail(format!("cannot start the async runtime: {err}"), 1))
}

/// Prints a markup line on standard error, as `err_console.print` does.
fn note(state: &mut CliState, markup: &str) {
    let text = console(true).print(markup).text;
    state.eprint(&text);
}

fn run_serve(state: &mut CliState, _path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let settings = Settings::from_config(&state.app()?.config()?);
    let host = option_text(m, "--host").unwrap_or_else(|| settings.host.clone());
    let port = port_option(m)?
        .filter(|port| *port != 0)
        .unwrap_or(settings.port);
    let allowed = option_flag(m, "--allow-remote") || settings.allow_non_loopback;
    if let Some(refusal) = bind_refusal(&host, allowed) {
        return Err(failed(refusal));
    }
    let lan_on = pair(m, "--lan").unwrap_or(settings.lan);
    let address = match lan_plan(
        lan_on,
        &host,
        settings.lan_address.as_deref(),
        wg_server::detect_lan_address,
    ) {
        LanPlan::Serve(address) => Some(address),
        LanPlan::NoAddress => {
            note(state, &format!("[yellow]{LOOPBACK_ONLY_NOTICE}[/]"));
            None
        }
        LanPlan::Off => None,
    };
    state.configure_logging()?;
    let token_file = state.app()?.paths.api_token_file().to_string();
    let home = state.home.clone();
    let with_mcp = pair(m, "--mcp").unwrap_or(true);
    let cancel = CancellationToken::new();
    let mcp = if with_mcp {
        let agent = Agent {
            name: "mcp-http-client".to_owned(),
            permissions: agent_permissions::<String>(&[])?,
        };
        let backend = Box::new(AppBackend {
            app: App::open(home.as_deref())?,
            features: FEATURES,
        });
        Some(http_router(agent, backend, cancel.clone())?)
    } else {
        None
    };
    let options = ServerOptions {
        host: host.clone(),
        port,
        lan: address.clone().map(|address| LanOptions {
            address,
            networks: settings.lan_networks.clone(),
            hostname: host_name(),
            pairing_code: None,
        }),
        ui: Ui::Embedded,
        mcp,
        routes: groups::routes_with(FEATURES),
    };
    let server = Arc::new(Server::new(options, App::open(home.as_deref())?)?);
    let runtime = runtime()?;
    let listeners = runtime
        .block_on(bind_listeners(&host, port, address.as_deref()))
        .map_err(|err| failed(err.to_string()))?;
    let background = if pair(m, "--worker").unwrap_or(true) {
        Some(start_worker(home)?)
    } else {
        None
    };
    note(state, &listening_notice(&host, port, &token_file));
    if let (Some(address), Some(code)) = (address.as_deref(), server.pairing_code()) {
        note(state, &local_network_markup(address, port, &code));
    }
    let served = runtime.block_on(async {
        tokio::select! {
            result = wg_server::serve(Arc::clone(&server), listeners) => result.map_err(|err| failed(err.to_string())),
            _ = tokio::signal::ctrl_c() => Ok(()),
        }
    });
    cancel.cancel();
    if let Some((stop, thread)) = background {
        stop.store(true, Ordering::SeqCst);
        let _ = thread.join();
    }
    served
}

type Background = (
    Arc<std::sync::atomic::AtomicBool>,
    std::thread::JoinHandle<()>,
);

/// `Worker(app)` on a thread named `workgraph-worker`, stopped through its flag.
fn start_worker(home: Option<std::path::PathBuf>) -> Result<Background, CliFailure> {
    let mut worker = Worker::open(
        WorkerOptions {
            home,
            ..WorkerOptions::default()
        },
        HandlerRegistry::from_features(FEATURES),
    )?;
    let stop = worker.stop_flag();
    let thread = std::thread::Builder::new()
        .name("workgraph-worker".to_owned())
        .spawn(move || {
            let _ = worker.run(RunOptions::default());
        })
        .map_err(|err| CliFailure::fail(format!("cannot start the worker: {err}"), 1))?;
    Ok((stop, thread))
}

fn run_mcp(state: &mut CliState, _path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let options = ServeOptions::parse(&mcp_arguments(m)).map_err(|err| {
        // An unknown `--grant` is a domain `permission_denied` (exit 5), as in Python; the other
        // flag mistakes stay usage errors (exit 2).
        if err.code == wg_types::ErrorCode::PermissionDenied {
            CliFailure::from(err)
        } else {
            failed(err.message)
        }
    })?;
    let token_file = state.app()?.paths.api_token_file().to_string();
    let home = state.home.clone();
    let agent = options.agent();
    let runtime = runtime()?;
    match options.transport {
        Transport::Stdio => {
            let backend = Box::new(AppBackend {
                app: App::open(home.as_deref())?,
                features: FEATURES,
            });
            Ok(runtime.block_on(serve_stdio(agent, backend))?)
        }
        Transport::Http => {
            let cancel = CancellationToken::new();
            let backend = Box::new(AppBackend {
                app: App::open(home.as_deref())?,
                features: FEATURES,
            });
            let mcp = http_router(agent, backend, cancel.clone())?;
            let server = Server::new(
                ServerOptions {
                    host: options.host.clone(),
                    port: options.port,
                    lan: None,
                    ui: Ui::Embedded,
                    mcp: Some(mcp),
                    routes: groups::routes_with(FEATURES),
                },
                App::open(home.as_deref())?,
            )?;
            let listeners = runtime
                .block_on(bind_listeners(&options.host, options.port, None))
                .map_err(|err| failed(err.to_string()))?;
            note(
                state,
                &format!(
                    "MCP Streamable HTTP on http://{}:{}/mcp/ (bearer token in {token_file})",
                    options.host, options.port
                ),
            );
            let served = runtime.block_on(async {
                tokio::select! {
                    result = wg_server::serve(Arc::new(server), listeners) => result.map_err(|err| failed(err.to_string())),
                    _ = tokio::signal::ctrl_c() => Ok(()),
                }
            });
            cancel.cancel();
            served
        }
    }
}
