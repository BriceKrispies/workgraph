//! IF-01: the skeleton of the `workgraph` binary. The contract-coverage test compares the clap
//! tree against `contracts/cli.json` for the groups listed as implemented (plan 5.5) and only
//! reports the others; the process tests run the real binary.

#![allow(clippy::expect_used)]

use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use wg_iface::cli::{Snapshot, diff_commands};

/// The implemented groups: the groups of `GROUPS`, then the features' groups in feature order.
/// The coverage test below derives its list from these, so no name is kept by hand.
fn all_groups() -> impl Iterator<Item = &'static wg_cli::commands::Group> {
    wg_cli::commands::GROUPS.iter().chain(
        wg_cli::commands::FEATURES
            .iter()
            .flat_map(|feature| feature.cli.iter()),
    )
}

/// The hand-written command of an implemented group.
fn implemented(name: &str) -> Option<clap::Command> {
    all_groups()
        .find(|group| group.name == name)
        .map(|group| (group.command)())
}

fn snapshot() -> Snapshot {
    Snapshot::parse(wg_iface::cli::tree::SNAPSHOT_JSON).expect("the snapshot parses")
}

/// Runs the binary against a fresh home of its own, so a command that writes never touches the
/// real data directory.
fn workgraph(args: &[&str]) -> Output {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "if01-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    Command::new(env!("CARGO_BIN_EXE_workgraph"))
        .args(args)
        .env("WORKGRAPH_HOME", &home)
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
    let root = wg_iface::cli::tree::root(&snapshot, &implemented);
    let names: Vec<String> = root
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect();
    assert_eq!(names, snapshot.top_level, "top-level commands and order");
    let implemented_names: Vec<&str> = all_groups().map(|group| group.name).collect();
    let mut reported = Vec::new();
    for sub in root.get_subcommands() {
        let expected = snapshot.group(sub.get_name());
        let actual = wg_iface::cli::tree::extract_group(sub);
        let diff = diff_commands(&expected, &actual);
        if implemented_names.contains(&sub.get_name()) {
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
    let root = wg_iface::cli::tree::root(&snapshot, &implemented);
    let actual = wg_iface::cli::tree::extract(&root);
    let expected: Vec<&_> = snapshot.commands.iter().collect();
    assert_eq!(snapshot.commands.len(), 192);
    assert_eq!(diff_commands(&expected, &actual), Vec::<String>::new());
}

#[test]
fn root_options_match_the_snapshot() {
    let snapshot = snapshot();
    let root = wg_iface::cli::tree::root(&snapshot, &implemented);
    assert_eq!(wg_iface::cli::tree::params_of(&root), snapshot.root_options);
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

// `analytics subagents` is implemented now, so the failure below is a real one: a fresh home has
// no default workspace (the same `not_found` the Python CLI raises).

#[test]
fn a_failing_command_reports_itself_in_human_mode() {
    let out = workgraph(&["analytics", "subagents"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(out.stdout.is_empty());
    let err = text(&out.stderr);
    assert!(err.starts_with("error (not_found):"), "{err}");
    assert!(err.contains("no default workspace configured"), "{err}");
}

#[test]
fn json_is_stripped_wherever_it_appears() {
    let expected = concat!(
        "{\"error\": {\"code\": \"not_found\", \"message\": ",
        "\"no default workspace configured; pass --workspace or run `workgraph workspace use <slug>`\", ",
        "\"details\": {}}}\n"
    );
    for args in [
        vec!["--json", "analytics", "subagents"],
        vec!["analytics", "--json", "subagents"],
        vec!["analytics", "subagents", "--json"],
    ] {
        let out = workgraph(&args);
        assert_eq!(out.status.code(), Some(4), "{args:?}");
        assert_eq!(text(&out.stdout), expected, "{args:?}");
        assert!(out.stderr.is_empty(), "{args:?}");
    }
    // An explicit, missing workspace names itself in the envelope, wherever `--json` sits.
    let out = workgraph(&[
        "--home",
        "somewhere",
        "-w",
        "main",
        "analytics",
        "subagents",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        text(&out.stdout).starts_with(
            "{\"error\": {\"code\": \"not_found\", \"message\": \"workspace 'main' not found\""
        ),
        "{}",
        text(&out.stdout)
    );
    assert!(out.stderr.is_empty());
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
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "agent n configured
"
    );
}
