//! The entry point: argv rules, parsing, dispatch and failure reporting (`main()` of
//! `cli/main.py` plus `handle_errors`).

use std::io::Write;

use clap::ArgMatches;
use clap::error::ErrorKind;
use wg_iface::cli::tree::{SNAPSHOT_JSON, root};
use wg_iface::cli::{CliFailure, Snapshot, not_implemented_message, report, split_json};

use crate::CliState;
use crate::commands;
use crate::output::console;

/// Exit code of a usage error (Click's, kept by clap).
const EXIT_USAGE: u8 = 2;

/// Runs `workgraph` with the process's arguments and streams and returns the exit code.
#[must_use]
pub fn run() -> u8 {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    if wg_iface::cli::is_hook_invocation(&args) {
        // `claude hook` never builds the clap tree, the console or the snapshot (P01, O15).
        return crate::hook::run(args.get(2..).unwrap_or_default());
    }
    run_with(
        args,
        Box::new(std::io::stdout()),
        Box::new(std::io::stderr()),
    )
}

/// [`run`] with explicit arguments (the program name excluded) and output streams.
#[must_use]
pub fn run_with(args: Vec<String>, stdout: Box<dyn Write>, stderr: Box<dyn Write>) -> u8 {
    let argv = split_json(args);
    let mut state = CliState {
        json: argv.json,
        home: None,
        workspace: None,
        app: None,
        stdout,
        stderr,
        console: console(false),
        err_console: console(true),
    };
    let snapshot = match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => snapshot,
        Err(err) => return fail(&mut state, &CliFailure::fail(err.to_string(), 1)),
    };
    let implemented = |name: &str| {
        commands::GROUPS
            .iter()
            .chain(
                commands::FEATURES
                    .iter()
                    .flat_map(|feature| feature.cli.iter()),
            )
            .find(|group| group.name == name)
            .map(|group| (group.command)())
    };
    let mut tree = root(&snapshot, &implemented);
    // Click's `no_args_is_help` looks at the raw arguments; clap counts `--home` filled from
    // `WORKGRAPH_HOME` as given, so an empty command line is answered here.
    if argv.args.is_empty() {
        state.print(&tree.render_help().to_string());
        return EXIT_USAGE;
    }
    let matches =
        match tree.try_get_matches_from(std::iter::once("workgraph".to_owned()).chain(argv.args)) {
            Ok(matches) => matches,
            Err(err) => return usage(&mut state, &err),
        };
    state.home = matches
        .get_one::<String>("--home")
        .map(std::path::PathBuf::from);
    state.workspace = matches.get_one::<String>("--workspace").cloned();
    match dispatch(&mut state, &matches) {
        Ok(()) => 0,
        Err(failure) => fail(&mut state, &failure),
    }
}

/// Prints clap's help, version or usage error. Help and version go to standard output with exit
/// 0; a group run without a subcommand prints its help to standard output and exits 2, as Click
/// 8.2 and later do; every other error goes to standard error with exit 2 (DV1: same content,
/// clap's layout).
fn usage(state: &mut CliState, err: &clap::Error) -> u8 {
    let text = err.render().to_string();
    match err.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            state.print(&text);
            0
        }
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            state.print(&text);
            EXIT_USAGE
        }
        _ => {
            state.eprint(&text);
            EXIT_USAGE
        }
    }
}

fn fail(state: &mut CliState, failure: &CliFailure) -> u8 {
    let printed = report(failure, state.json, &console(true));
    state.print(&printed.stdout);
    state.eprint(&printed.stderr);
    printed.code
}

/// Walks to the matched leaf command and runs its group's handler, or reports that the command
/// is not implemented yet.
fn dispatch(state: &mut CliState, matches: &ArgMatches) -> Result<(), CliFailure> {
    let mut path = Vec::new();
    let mut leaf = matches;
    while let Some((name, sub)) = leaf.subcommand() {
        path.push(name.to_owned());
        leaf = sub;
    }
    let group = path.first().and_then(|name| {
        commands::GROUPS
            .iter()
            .chain(
                commands::FEATURES
                    .iter()
                    .flat_map(|feature| feature.cli.iter()),
            )
            .find(|group| group.name == name)
    });
    match group {
        Some(group) => (group.run)(state, &path, leaf),
        None => Err(CliFailure::fail(not_implemented_message(&path), 1)),
    }
}
