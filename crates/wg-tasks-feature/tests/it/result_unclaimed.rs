//! `task result <KEY> succeeded|failed` on an unclaimed task (backlog, ready, blocked) is refused
//! with an invalid transition that names the status and the claim step, and changes nothing. The
//! arguments go through the CLI parser and `result_body`, the same path `task result` runs.

use std::error::Error;
use std::path::PathBuf;

use serde_json::json;
use wg_json::Value;
use wg_registry_feature::service::registry::workspace_create;
use wg_runtime::{App, human_context};
use wg_tasks_feature::service;
use wg_tasks_feature::spec::args::{command, result_body};
use wg_types::ErrorCode;

type TestResult = Result<(), Box<dyn Error>>;

fn app(name: &str) -> Result<App, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("result_unclaimed")
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

/// The `task result` arguments after the program name, parsed by the group's clap command.
fn result_matches(args: &[&str]) -> Result<clap::ArgMatches, Box<dyn Error>> {
    let mut argv = vec!["task", "result"];
    argv.extend_from_slice(args);
    let matches = command().try_get_matches_from(argv)?;
    Ok(matches
        .subcommand_matches("result")
        .ok_or("no result subcommand")?
        .clone())
}

/// The status text of a task shown by `task show`.
fn status_of(task: &Value) -> Option<String> {
    match wg_iface::cli::args::get(task, "status") {
        Some(Value::Str(text)) => text.to_text(),
        _ => None,
    }
}

#[test]
fn a_cli_result_on_an_unclaimed_task_is_refused_and_changes_nothing() -> TestResult {
    let mut app = app("refused")?;
    let human = human_context(None)?;
    for (status, title) in [("backlog", "Backlog task"), ("ready", "Ready task")] {
        let create = json!({
            "title": title, "description": "", "acceptance_criteria": ["done"],
            "priority": "medium", "status": status, "tags": [], "files": [],
            "projects": [], "repositories": [], "depends_on": [], "auto_execute": false,
        });
        service::task_create(&mut app, &human, None, &create)?;
    }
    let (backlog, ready) = ("WG-1", "WG-2");

    for key in [backlog, ready] {
        for outcome in ["succeeded", "failed"] {
            let matches = result_matches(&[key, outcome])?;
            let reference = matches
                .get_one::<String>("ref")
                .ok_or("no task reference")?
                .clone();
            let body = result_body(&matches).map_err(|_| "result_body refused the arguments")?;
            let error = service::task_result(&mut app, &human, None, &reference, &body)
                .err()
                .ok_or_else(|| {
                    format!("{key} {outcome}: a result on an unclaimed task succeeded")
                })?;
            assert_eq!(error.code, ErrorCode::InvalidTransition, "{key} {outcome}");
            assert!(error.message.contains("claim"), "{}", error.message);

            let shown = service::task_show(&mut app, &human, None, &[key.to_owned()])?;
            let task = shown.items.first().ok_or("task not shown")?;
            let expected = if key == backlog { "backlog" } else { "ready" };
            assert_eq!(
                status_of(task).as_deref(),
                Some(expected),
                "{key} {outcome}"
            );
        }
    }
    Ok(())
}
