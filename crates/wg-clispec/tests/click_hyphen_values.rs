//! A value-taking option takes the token after it as its value even when that token starts with
//! `-`, as Click does (`agent add-command --arg --mode`); positionals and flags are unchanged.

#![allow(clippy::expect_used)]

use wg_clispec::Snapshot;
use wg_clispec::args::many;
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};

// covers: C001
#[test]
fn option_value_may_start_with_a_dash() {
    let snapshot = Snapshot::parse(SNAPSHOT_JSON).expect("the snapshot parses");
    let agent = placeholder(&snapshot, "agent");
    let matches = agent
        .try_get_matches_from([
            "agent",
            "add-command",
            "bot",
            "/bin/bot",
            "--arg",
            "--mode",
            "--arg",
            "-x",
        ])
        .expect("the dash-led values are option values");
    let (name, add) = matches.subcommand().expect("the add-command subcommand");
    assert_eq!(name, "add-command");
    assert_eq!(many(add, "--arg"), ["--mode", "-x"]);
}

// covers: C001
#[test]
fn an_option_still_needs_its_value() {
    let snapshot = Snapshot::parse(SNAPSHOT_JSON).expect("the snapshot parses");
    let agent = placeholder(&snapshot, "agent");
    let result = agent.try_get_matches_from(["agent", "add-command", "bot", "/bin/bot", "--arg"]);
    assert!(result.is_err(), "--arg without a value must still fail");
}
