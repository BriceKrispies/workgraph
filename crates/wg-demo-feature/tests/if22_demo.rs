//! IF-22: the `demo` group's clap shape and the pure data behind `demo e2e` and `demo seed`.

use wg_demo_feature::spec;

// covers: C063
#[test]
fn demo_group_has_e2e_and_seed() {
    let names: Vec<String> = spec::command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    assert_eq!(names, ["e2e", "seed"]);
}

// covers: C064
#[test]
fn seed_defaults_to_demo_workspace_without_force() {
    let matches = spec::command()
        .try_get_matches_from(["demo", "seed"])
        .expect("demo seed parses");
    let (name, sub) = matches.subcommand().expect("a subcommand");
    assert_eq!(name, "seed");
    assert_eq!(spec::seed_args(sub), ("demo".to_owned(), false));
}

// covers: C063
#[test]
fn e2e_takes_home_and_keep() {
    let matches = spec::command()
        .try_get_matches_from(["demo", "e2e", "--home", "x", "--keep"])
        .expect("demo e2e parses");
    let (_, sub) = matches.subcommand().expect("a subcommand");
    let (home, keep) = spec::e2e_args(sub);
    assert_eq!(home, Some(std::path::PathBuf::from("x")));
    assert!(keep);
}

#[test]
fn plan_document_is_python_json_dumps() {
    assert_eq!(
        spec::plan_document(&spec::REPOS[0]),
        "{\"file\": \"auth/refresh.py\", \"content\": \"# {task}: refresh token issuance\\nMARKER = 'WORKGRAPH_DEMO_OK'\\n\\n\\ndef issue_refresh_token(user: str) -> str:\\n    return f'refresh-{user}'\\n\\n\\nclass RefreshPolicy:\\n    ttl_seconds = 3600\\n\"}"
    );
}

#[test]
fn seed_table_matches_python_order_and_projects() {
    assert_eq!(spec::SEED_TASKS.len(), 7);
    assert_eq!(spec::SEED_TASKS[2].agent, "scripted-buggy");
    assert!(spec::SEED_TASKS[2].retry);
    assert_eq!(spec::seed_project(&["web-client"]), "frontend");
    assert_eq!(
        spec::seed_project(&["auth-service", "web-client"]),
        "platform"
    );
}

#[test]
fn agent_arguments_follow_the_manifest_placeholder() {
    assert_eq!(
        spec::agent_args("buggy"),
        [
            "--mode".to_owned(),
            "buggy".to_owned(),
            "{manifest}".to_owned()
        ]
    );
}
