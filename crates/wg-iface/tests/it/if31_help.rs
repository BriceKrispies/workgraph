//! IF-31: `--help` lists every option a command's snapshot declares, and the `task` options carry
//! their one-line descriptions.

#![allow(clippy::expect_used)]

use wg_iface::cli::tree::root;
use wg_iface::cli::{Command, Kind, Snapshot};

fn snapshot() -> Snapshot {
    Snapshot::parse(include_str!("../../../../contracts/cli.json")).expect("parses")
}

/// The help text of the leaf command at `path` in the snapshot's own command tree.
fn help_of(snapshot: &Snapshot, path: &[String]) -> String {
    let tree = root(snapshot, &|_| None);
    let mut cmd: &Command = &tree;
    for segment in path {
        cmd = cmd
            .find_subcommand(segment)
            .unwrap_or_else(|| panic!("no command {segment} on {path:?}"));
    }
    cmd.clone().render_help().to_string()
}

#[test]
fn every_option_of_every_command_is_listed_in_its_help() {
    let snapshot = snapshot();
    for command in &snapshot.commands {
        let help = help_of(&snapshot, &command.path);
        for param in command.params.iter().filter(|p| p.kind == Kind::Option) {
            for opt in param.opts.iter().filter(|o| o.starts_with("--")) {
                assert!(
                    help.contains(opt.as_str()),
                    "`{}` accepts {opt} but its --help does not list it:\n{help}",
                    command.display()
                );
            }
        }
    }
}

#[test]
fn every_task_option_has_a_description() {
    let snapshot = snapshot();
    for command in snapshot.group("task") {
        for param in command.params.iter().filter(|p| p.kind == Kind::Option) {
            assert!(
                param.help.as_deref().is_some_and(|h| !h.trim().is_empty()),
                "`{}` option {} has no description",
                command.display(),
                param.key()
            );
        }
    }
}

#[test]
fn task_accept_help_describes_manual_reason_and_promote_dependents() {
    let snapshot = snapshot();
    let help = help_of(&snapshot, &["task".to_owned(), "accept".to_owned()]);
    for (flag, text) in [
        ("--manual", "Accept by hand, without a verification run."),
        ("--reason", "Reason recorded with the acceptance."),
        ("--promote-dependents", "Also move the backlog dependents"),
    ] {
        let line = help
            .lines()
            .find(|line| {
                line.split_whitespace()
                    .any(|word| word.trim_end_matches(',') == flag)
            })
            .unwrap_or_else(|| panic!("{flag} missing from:\n{help}"));
        assert!(line.contains(text), "{flag} has no description: {line}");
    }
}
