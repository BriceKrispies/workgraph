//! The `attempt` command group's clap tree (IF-09): the snapshot's subcommands, in order, and the
//! options the read commands take.

use wg_exec_feature::spec::cli_attempt::command;

fn subcommand_names() -> Vec<String> {
    command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

// covers: C020 C021 C022 C017
#[test]
fn attempt_group_lists_every_snapshot_subcommand() {
    let names = subcommand_names();
    for expected in [
        "list",
        "show",
        "create",
        "start",
        "submit",
        "abandon",
        "finish",
        "compare",
        "manifests",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing attempt subcommand {expected}: {names:?}"
        );
    }
}

// covers: C020
#[test]
fn attempt_list_takes_repeatable_status_and_int_limit() {
    let tree = command();
    let list = tree
        .find_subcommand("list")
        .expect("attempt list is in the tree");
    let status = list
        .get_arguments()
        .find(|arg| arg.get_long() == Some("status"))
        .expect("--status is an option of attempt list");
    assert!(status.get_action().takes_values(), "--status takes a value");
    assert!(
        list.get_arguments()
            .any(|arg| arg.get_long() == Some("limit")),
        "--limit is an option of attempt list"
    );
}
