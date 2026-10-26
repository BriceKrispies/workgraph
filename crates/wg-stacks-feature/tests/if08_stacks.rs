//! The `stack` command tree (C135 to C144) and the detail summary of a stack item.

use wg_stacks_feature::spec::{command, view::detail_summary};

// covers: C135
#[test]
fn stack_add_takes_parent_and_task() {
    let matches = command()
        .try_get_matches_from(["stack", "add", "WG-1", "WG-2", "--workspace", "main"])
        .expect("parses");
    let (name, sub) = matches.subcommand().expect("a subcommand");
    assert_eq!(name, "add");
    assert_eq!(sub.get_one::<String>("parent").map(String::as_str), Some("WG-1"));
    assert_eq!(sub.get_one::<String>("task").map(String::as_str), Some("WG-2"));
}

// covers: C136
#[test]
fn stack_create_takes_only_the_parent() {
    let matches = command()
        .try_get_matches_from(["stack", "create", "WG-1"])
        .expect("parses");
    let (name, _) = matches.subcommand().expect("a subcommand");
    assert_eq!(name, "create");
    assert!(command().try_get_matches_from(["stack", "create"]).is_err());
}

// covers: C137
#[test]
fn stack_eject_needs_a_reason() {
    let matches = command()
        .try_get_matches_from(["stack", "eject", "WG-1", "WG-2", "-m", "fails"])
        .expect("parses");
    let (_, sub) = matches.subcommand().expect("a subcommand");
    assert_eq!(sub.get_one::<String>("--reason").map(String::as_str), Some("fails"));
    assert!(
        command()
            .try_get_matches_from(["stack", "eject", "WG-1", "WG-2"])
            .is_err()
    );
}

// covers: C138
// covers: C139
// covers: C141
// covers: C142
// covers: C143
// covers: C144
#[test]
fn stack_commands_take_the_parent_and_the_workspace() {
    for name in ["history", "integrate", "show", "verify"] {
        let matches = command()
            .try_get_matches_from(["stack", name, "WG-1", "-w", "main"])
            .expect("parses");
        let (found, sub) = matches.subcommand().expect("a subcommand");
        assert_eq!(found, name);
        assert_eq!(sub.get_one::<String>("parent").map(String::as_str), Some("WG-1"));
    }
    for name in ["readmit", "remove"] {
        let matches = command()
            .try_get_matches_from(["stack", name, "WG-1", "WG-2"])
            .expect("parses");
        let (found, _) = matches.subcommand().expect("a subcommand");
        assert_eq!(found, name);
    }
}

// covers: C140
#[test]
fn stack_list_takes_all() {
    let matches = command()
        .try_get_matches_from(["stack", "list", "--all"])
        .expect("parses");
    let (name, sub) = matches.subcommand().expect("a subcommand");
    assert_eq!(name, "list");
    assert!(sub.get_flag("--all"));
}

#[test]
fn detail_summary_names_conflicts_and_ejections() {
    let detail = r#"{"merges": {}, "conflicts": {"svc": {"paths": ["a.txt", "b.txt"]}}, "ejection": {"position": 2, "failed_checks": ["unit", "lint"]}}"#;
    assert_eq!(
        detail_summary(Some(detail)),
        "conflict in svc: a.txt, b.txt; ejected at position 2: unit, lint"
    );
}

#[test]
fn detail_summary_reports_a_conflict_without_paths() {
    let detail = r#"{"conflicts": {"svc": {"paths": []}}}"#;
    assert_eq!(detail_summary(Some(detail)), "conflict in svc: no paths reported");
}

#[test]
fn detail_summary_is_empty_without_a_detail_and_shows_plain_text_as_it_is() {
    assert_eq!(detail_summary(None), "");
    assert_eq!(detail_summary(Some("")), "");
    assert_eq!(detail_summary(Some("not json")), "not json");
    assert_eq!(detail_summary(Some("[1, 2]")), "[1, 2]");
}
