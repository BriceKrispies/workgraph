//! IF-01: the skeleton of the `workgraph` binary. The contract-coverage test compares the clap
//! tree against `contracts/cli.json` for the groups listed as implemented (plan 5.5) and only
//! reports the others; the process tests run the real binary.

#![allow(clippy::expect_used)]

use std::process::{Command, Output};

use wg_clispec::{Snapshot, diff_commands};

/// Top-level groups whose clap tree must equal the snapshot. Each group task adds its name.
const IMPLEMENTED: &[&str] = &["config", "doctor", "init", "task"];

/// The hand-written command of an implemented group.
fn implemented(name: &str) -> Option<clap::Command> {
    wg_cli::commands::GROUPS
        .iter()
        .find(|group| group.name == name)
        .map(|group| (group.command)())
}

fn snapshot() -> Snapshot {
    Snapshot::parse(wg_clispec::tree::SNAPSHOT_JSON).expect("the snapshot parses")
}

fn workgraph(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .args(args)
        .env("COLUMNS", "120")
        .output()
        .expect("the binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn clap_tree_matches_the_snapshot_for_implemented_groups() {
    let snapshot = snapshot();
    let root = wg_clispec::tree::root(&snapshot, &implemented);
    let names: Vec<String> = root
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect();
    assert_eq!(names, snapshot.top_level, "top-level commands and order");
    let mut reported = Vec::new();
    for sub in root.get_subcommands() {
        let expected = snapshot.group(sub.get_name());
        let actual = wg_clispec::tree::extract_group(sub);
        let diff = diff_commands(&expected, &actual);
        if IMPLEMENTED.contains(&sub.get_name()) {
            assert!(diff.is_empty(), "{}: {diff:#?}", sub.get_name());
        } else {
            reported.push(sub.get_name().to_owned());
        }
    }
    eprintln!("not implemented, reported only: {}", reported.join(" "));
}

#[test]
fn placeholders_cover_every_snapshot_command() {
    // The placeholders are built from the snapshot, so the whole tree must agree with it.
    let snapshot = snapshot();
    let root = wg_clispec::tree::root(&snapshot, &implemented);
    let actual = wg_clispec::tree::extract(&root);
    let expected: Vec<&_> = snapshot.commands.iter().collect();
    assert_eq!(snapshot.commands.len(), 192);
    assert_eq!(diff_commands(&expected, &actual), Vec::<String>::new());
}

#[test]
fn root_options_match_the_snapshot() {
    let snapshot = snapshot();
    let root = wg_clispec::tree::root(&snapshot, &implemented);
    assert_eq!(wg_clispec::tree::params_of(&root), snapshot.root_options);
}

#[test]
fn version_prints_and_exits_zero() {
    let out = workgraph(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout), "workgraph 0.1.0\n");
}

#[test]
fn no_arguments_print_the_root_help() {
    let out = workgraph(&[]);
    assert_eq!(out.status.code(), Some(2));
    let help = text(&out.stdout);
    assert!(help.contains("Usage: workgraph"), "{help}");
    for name in ["task", "--home", "--workspace", "--version"] {
        assert!(help.contains(name), "{name} in {help}");
    }
    assert!(out.stderr.is_empty());
}

#[test]
fn a_group_without_a_subcommand_prints_its_help() {
    let out = workgraph(&["task"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stdout).contains("list"));
}

#[test]
fn unknown_option_is_a_usage_error() {
    let out = workgraph(&["--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("--bogus"));
}

#[test]
fn missing_argument_is_a_usage_error() {
    let out = workgraph(&["task", "show"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("REFS"));
}

#[test]
fn unimplemented_command_reports_itself_in_human_mode() {
    let out = workgraph(&["task", "claim", "WG-1"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err = text(&out.stderr);
    assert!(err.starts_with("error:"), "{err}");
    assert!(err.contains("workgraph task claim"), "{err}");
}

#[test]
fn json_is_stripped_wherever_it_appears() {
    let expected = concat!(
        "{\"error\": {\"code\": \"cli_error\", \"message\": ",
        "\"`workgraph task claim` is not implemented in this build of the Rust rewrite yet\", ",
        "\"details\": {}}}\n"
    );
    for args in [
        vec!["--json", "task", "claim", "WG-1"],
        vec!["task", "--json", "claim", "WG-1"],
        vec!["task", "claim", "WG-1", "--json"],
        vec![
            "--home",
            "somewhere",
            "-w",
            "main",
            "task",
            "claim",
            "WG-1",
            "--json",
        ],
    ] {
        let out = workgraph(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&out.stdout), expected, "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn options_with_negated_spellings_parse() {
    let out = workgraph(&[
        "agent",
        "add-command",
        "n",
        "exe",
        "--no-supports-usage",
        "--arg",
        "x",
    ]);
    // It parses, then reports as unimplemented.
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("not implemented"));
}
