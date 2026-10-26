//! `task create -d` takes a description that starts with `-`, as Click does (`-d '- step one'`).

#![allow(clippy::expect_used)]

use wg_clispec::args::one;

// covers: C152
#[test]
fn description_may_start_with_a_dash() {
    let command = wg_tasks_feature::spec::args::command();
    let matches = command
        .try_get_matches_from(["task", "create", "x", "-d", "- step one"])
        .expect("a dash-led description is an option value");
    let (name, create) = matches.subcommand().expect("the create subcommand");
    assert_eq!(name, "create");
    assert_eq!(one(create, "--description").as_deref(), Some("- step one"));
}
