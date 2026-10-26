//! `workgraph init`, `doctor` and `config show, path, validate, migrate, set` (IF-02).
//!
//! The clap commands are the snapshot's own; each handler reads the options with
//! `wg_clispec::setup`, calls one `App` entry point and prints the result.

use clap::ArgMatches;
use wg_clispec::setup as view;
use wg_clispec::task::option_text;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{InitRequest, human_context};

use super::Group;
use crate::CliState;

/// The `init` command.
pub const INIT: Group = Group {
    name: "init",
    command: view::init_command,
    run: init,
};

/// The `doctor` command.
pub const DOCTOR: Group = Group {
    name: "doctor",
    command: view::doctor_command,
    run: doctor,
};

/// The `config` group.
pub const CONFIG: Group = Group {
    name: "config",
    command: view::config_command,
    run: config,
};

fn init(state: &mut CliState, _path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let options = view::init_options(m);
    let context = human_context(None)?;
    let request = InitRequest {
        workspace: &options.workspace,
        name: options.name.as_deref(),
        prefix: &options.prefix,
        no_workspace: options.no_workspace,
    };
    let out = state.app()?.init(&context, &request)?;
    state.emit(&out, Some(&|c| view::init_view(c, &out)))
}

fn doctor(state: &mut CliState, _path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let languages = wg_codeparse::available_languages()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let diagnosis = state.app()?.doctor(languages, view::verify_events(m))?;
    let out = diagnosis.result;
    state.emit(&out, Some(&|c| view::doctor_view(c, &out)))?;
    if diagnosis.healthy {
        Ok(())
    } else {
        Err(CliFailure::Exit(1))
    }
}

fn config(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    match path.get(1).map(String::as_str) {
        Some("show") => {
            let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
            let show = state.app()?.config_show(workspace.as_deref())?;
            let out = view::show_value(&show);
            state.emit(&out, Some(&|_| view::show_view(&show)))
        }
        Some("path") => {
            let out = state.app()?.config_paths();
            state.emit(&out, Some(&|c| view::paths_view(c, &out)))
        }
        Some("validate") => {
            let problems = state.app()?.config_problems()?;
            let out = view::validate_value(&problems);
            state.emit(&out, Some(&|c| view::validate_view(c, &out)))?;
            if problems.is_empty() {
                Ok(())
            } else {
                Err(CliFailure::Exit(7))
            }
        }
        Some("migrate") => {
            let out = state.app()?.config_migrate()?;
            state.emit(&out, Some(&|c| view::migrate_view(c, &out)))
        }
        Some("set") => set(state, m),
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}

/// `config set KEY VALUE [--workspace SLUG]`: the value is validated before anything is written.
fn set(state: &mut CliState, m: &ArgMatches) -> Result<(), CliFailure> {
    let key = option_text(m, "key").unwrap_or_default();
    let raw = m
        .try_get_one::<String>("value")
        .ok()
        .flatten()
        .cloned()
        .unwrap_or_default();
    let workspace = option_text(m, "--workspace");
    let app = state.app()?;
    let base = app.config_set_base(workspace.as_deref())?;
    let change = view::config_set(&base, &key, &raw, workspace.is_some())?;
    match workspace.as_deref() {
        Some(slug) => {
            app.save_workspace_overrides(slug, &change.document)?;
            app.effective_config(Some(slug))?;
        }
        None => app.save_config(&change.document)?,
    }
    let out = view::config_set_value(&key, &change.parsed, workspace.as_deref());
    state.emit(
        &out,
        Some(&|c| view::config_set_line(c, &key, &change.parsed)),
    )
}
