//! `task update -c/--criterion` replaces the acceptance criteria, and `--add-criterion` appends to
//! them after reading the task. Both options together are a usage error.

use std::error::Error;
use std::path::PathBuf;

use serde_json::json;
use wg_iface::cli::CliFailure;
use wg_registry_feature::service::registry::workspace_create;
use wg_runtime::{App, human_context};
use wg_tasks_feature::service;
use wg_tasks_feature::spec::args::{command, update_body};

type TestResult = Result<(), Box<dyn Error>>;

fn app(name: &str) -> Result<App, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("update_criteria")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let mut app = App::open(Some(&dir))?;
    let human = human_context(None)?;
    let body = json!({"slug": "main", "name": "Main", "description": "", "key_prefix": "WG"});
    workspace_create(&mut app, &human, &body, true)?;
    Ok(app)
}

/// `task update` arguments after the program name, parsed by the group's clap command.
fn update_matches(args: &[&str]) -> Result<clap::ArgMatches, Box<dyn Error>> {
    let mut argv = vec!["task", "update"];
    argv.extend_from_slice(args);
    let matches = command().try_get_matches_from(argv)?;
    Ok(matches
        .subcommand_matches("update")
        .ok_or("no update subcommand")?
        .clone())
}

fn no_file(_: &str) -> Result<String, String> {
    Err("no file read in this test".to_owned())
}

#[test]
fn criterion_replaces_the_list_and_add_criterion_appends_to_it() -> TestResult {
    let mut app = app("replace_and_append")?;
    let human = human_context(None)?;
    let create = json!({
        "title": "Criteria", "description": "", "acceptance_criteria": ["first"],
        "priority": "medium", "status": "backlog", "tags": [], "files": [],
        "projects": [], "repositories": [], "depends_on": [], "auto_execute": false,
    });
    service::task_create(&mut app, &human, None, &create)?;

    // --add-criterion: the current list read from the task, then the added value.
    let appended =
        service::task_append_criteria(&mut app, &human, None, "WG-1", &["second".to_owned()])?;
    assert_eq!(appended, vec!["first", "second"]);
    service::task_update(
        &mut app,
        &human,
        None,
        "WG-1",
        &json!({"acceptance_criteria": appended}),
    )?;
    let shown = service::task_show(&mut app, &human, None, &["WG-1".to_owned()])?;
    let task = shown.items.first().ok_or("WG-1 not shown")?;
    let stored = wg_iface::cli::args::items(wg_iface::cli::args::get(task, "acceptance_criteria"));
    assert_eq!(stored.len(), 2);

    // -c: the body carries exactly the given values, replacing the list.
    let matches = update_matches(&["WG-1", "-c", "only", "--criterion", "other"])?;
    let body = update_body(&matches, &no_file).map_err(|_| "update_body refused replace")?;
    assert_eq!(body["acceptance_criteria"], json!(["only", "other"]));
    Ok(())
}

#[test]
fn criterion_and_add_criterion_together_are_a_usage_error() -> TestResult {
    let matches = update_matches(&["WG-1", "-c", "a", "--add-criterion", "b"])?;
    let failure = update_body(&matches, &no_file);
    assert!(matches!(failure, Err(CliFailure::Usage { .. })));
    Ok(())
}
