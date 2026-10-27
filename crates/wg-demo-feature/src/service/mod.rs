//! The demo commands' glue (Shell): the steps that open the store, write the demo repositories
//! and call the feature services of the other crates. The decisions (the names, the files, the
//! checks, the seed table) are in [`crate::spec`].
//!
//! Not ported yet, so the commands stop at the first step that needs them and say so: the
//! verification configs of `demo/scenario.py` step 3 and of `demo seed` (no create service in
//! `wg-verify-feature`), the classification requests (`wg-intel-feature` has no request service),
//! the search embeddings and the retrospective report (no service in the analytics feature), and
//! the MCP and HTTP steps of the demonstration. `demo seed` checks its preconditions and stops
//! before it writes anything, so a refused run leaves the home as it was.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use wg_clispec::CliFailure;
use wg_clispec::args::{get, items, s};
use wg_config::set_path;
use wg_git_feature::service::git_sync;
use wg_json::Value;
use wg_registry_feature::service::{repo_list, repo_register, workspace_create, workspace_list};
use wg_registry_feature::spec::registry::workspace_create_body;
use wg_registry_feature::spec::repos::RegisterInput;
use wg_runtime::{App, human_context};

use crate::spec::{self, REPOS, RepoSpec, step_line, written};

/// What `demo seed` fails with once its preconditions hold: the seeding services are not ported.
const SEED_NOT_PORTED: &str = "demo seed stops before it writes anything: verification configs, classification requests, search embeddings and the retrospective report are not ported yet";

/// A fresh directory for `demo e2e` without `--home` (`tempfile.mkdtemp(prefix="workgraph-demo-")`).
///
/// # Errors
///
/// Returns a CLI failure when the directory cannot be created.
pub fn fresh_temp_home() -> Result<PathBuf, CliFailure> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let path = std::env::temp_dir().join(format!("workgraph-demo-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path)
        .map_err(|err| CliFailure::fail(format!("cannot create {}: {err}", path.display()), 1))?;
    Ok(path)
}

/// `demo seed`: the preconditions of `demo/seed.py`'s command (exit 3 on a workspace that exists,
/// or on data without `--force`), then the refusal that the seeding services are not ported.
///
/// # Errors
///
/// Always an error for now: the precondition failures, or [`SEED_NOT_PORTED`].
pub fn seed(app: &mut App, workspace: &str, force: bool) -> Result<Value, CliFailure> {
    let context = human_context(Some("demo"))?;
    let listing = workspace_list(app, &context, false)?;
    let existing: Vec<String> = items(Some(&listing))
        .iter()
        .map(|row| s(get(row, "slug")))
        .collect();
    if existing.iter().any(|slug| slug == workspace) {
        return Err(CliFailure::fail(
            format!("workspace '{workspace}' already exists"),
            3,
        ));
    }
    if !existing.is_empty() && !force {
        return Err(CliFailure::fail(spec::SEED_HAS_DATA, 3));
    }
    Err(CliFailure::fail(SEED_NOT_PORTED, 1))
}

/// `demo e2e` in `home`: steps 1 and 2 of `demo/scenario.py` (the workspace and the three Git
/// repositories), each recorded in `lines` as `[ n] ok  title`. Step 3 needs verification
/// configs, which are not ported, so the demonstration fails there and the home is kept.
///
/// # Errors
///
/// Returns the first step that fails, and always fails at step 3 for now.
pub fn e2e(home: &Path, lines: &mut Vec<String>) -> Result<(), CliFailure> {
    let mut app = App::open(Some(home))?;
    let operator = human_context(Some("demo-operator"))?;

    configure_demo(
        &mut app,
        &[("scripted", "success"), ("scripted-buggy", "buggy")],
    )?;
    workspace_create(
        &mut app,
        &operator,
        &workspace_create_body("demo", Some("Demo"), "WG", ""),
        true,
    )?;
    app.api_token(true)?;
    lines.push(step_line(1, spec::STEP_WORKSPACE));

    let paths = create_repositories(&home.join("repos"))?;
    for (_, path) in &paths {
        let input = RegisterInput {
            path: path.display().to_string(),
            ..RegisterInput::default()
        };
        repo_register(&mut app, &operator, Some("demo"), &input)?;
    }
    let mut names: Vec<String> = repo_list(&mut app, &operator, Some("demo"), false, None)?
        .iter()
        .map(|row| s(get(row, "name")))
        .collect();
    names.sort();
    let mut expected: Vec<&str> = REPOS.iter().map(|repo| repo.name).collect();
    expected.sort_unstable();
    if names != expected {
        return Err(CliFailure::fail(
            format!("demonstration failed: registered {names:?}"),
            1,
        ));
    }
    for repo in &REPOS {
        git_sync(&mut app, &operator, Some("demo"), Some(repo.name), false)?;
    }
    lines.push(step_line(2, spec::STEP_REPOSITORIES));

    Err(CliFailure::fail(
        format!(
            "demonstration failed: step 3 ({}) needs verification configs, which are not ported yet (home kept at {})",
            spec::STEP_PROJECT,
            home.display()
        ),
        1,
    ))
}

/// The demo's configuration (step 1): heuristic inference, the execution switch, the scripted
/// agents with the demo binary (DV4) and manual acceptance.
fn configure_demo(app: &mut App, agents: &[(&str, &str)]) -> Result<(), CliFailure> {
    let mut config = app.config()?;
    let settings: [(&str, serde_json::Value); 6] = [
        (
            "llm.classification",
            serde_json::json!({"provider": "heuristic", "model": "heuristic-v1"}),
        ),
        (
            "llm.embeddings",
            serde_json::json!({"provider": "heuristic", "model": "hash-embed-256"}),
        ),
        (
            "llm.summarization",
            serde_json::json!({"provider": "heuristic", "model": "heuristic-v1"}),
        ),
        ("execution.enabled", serde_json::json!(true)),
        ("execution.default_agent", serde_json::json!("scripted")),
        ("verification.auto_accept", serde_json::json!(false)),
    ];
    for (key, value) in settings {
        config = set_path(&config, key, value).ok_or_else(|| {
            CliFailure::fail(format!("cannot set {key}: not a table in config.toml"), 1)
        })?;
    }
    let executable = demo_agent_path()?;
    for (name, mode) in agents {
        let value = serde_json::json!({
            "executable": executable,
            "args": spec::agent_args(mode),
            "supports_usage": true,
        });
        config = set_path(&config, &format!("agents.{name}"), value)
            .ok_or_else(|| CliFailure::fail(format!("cannot set agents.{name}: not a table"), 1))?;
    }
    app.save_config(&config)?;
    Ok(())
}

/// The `workgraph-demo-agent` binary next to the running `workgraph` (DV4).
fn demo_agent_path() -> Result<String, CliFailure> {
    let exe = std::env::current_exe()
        .map_err(|err| CliFailure::fail(format!("cannot locate workgraph: {err}"), 1))?;
    let dir = exe.parent().map_or_else(PathBuf::new, Path::to_path_buf);
    let binary = format!(
        "{}{}",
        spec::DEMO_AGENT_BINARY,
        std::env::consts::EXE_SUFFIX
    );
    Ok(dir.join(binary).display().to_string())
}

/// `create_repositories` of `demo/scenario.py`: one repository per [`REPOS`] entry under `root`,
/// initialised on `main`, seeded, and committed once as the human operator.
fn create_repositories(root: &Path) -> Result<Vec<(&'static str, PathBuf)>, CliFailure> {
    let mut paths = Vec::new();
    for repo in &REPOS {
        let path = root.join(repo.name);
        create_repository(&path, repo)?;
        paths.push((repo.name, path));
    }
    Ok(paths)
}

fn create_repository(path: &Path, repo: &RepoSpec) -> Result<(), CliFailure> {
    written(std::fs::create_dir_all(path), path)?;
    git(path, &["init", "-q", "-b", "main"])?;
    git(path, &["config", "core.autocrlf", "false"])?;
    let seed = path.join(repo.seed_file);
    if let Some(parent) = seed.parent() {
        written(std::fs::create_dir_all(parent), parent)?;
    }
    written(std::fs::write(&seed, repo.seed), &seed)?;
    let plan = path.join("workgraph-demo.json");
    written(std::fs::write(&plan, spec::plan_document(repo)), &plan)?;
    let check = path.join("check.py");
    written(std::fs::write(&check, spec::CHECK_SCRIPT), &check)?;
    let readme = path.join("README.md");
    written(std::fs::write(&readme, spec::readme(repo.name)), &readme)?;
    git(path, &["add", "-A"])?;
    git(
        path,
        &[
            "commit",
            "-q",
            "-m",
            &spec::initial_commit_message(repo.name),
        ],
    )
}

/// `git -C <path> ...` with the demo's author identity, output captured as `_git` does.
fn git(path: &Path, args: &[&str]) -> Result<(), CliFailure> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Demo Human")
        .env("GIT_AUTHOR_EMAIL", "human@workgraph.local")
        .env("GIT_COMMITTER_NAME", "Demo Human")
        .env("GIT_COMMITTER_EMAIL", "human@workgraph.local")
        .output()
        .map_err(|err| CliFailure::fail(format!("cannot run git: {err}"), 1))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(CliFailure::fail(
            format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            1,
        ))
    }
}
