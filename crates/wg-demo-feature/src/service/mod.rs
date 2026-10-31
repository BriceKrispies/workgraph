//! The demo commands' glue (Shell): the steps that open the store, write the demo repositories
//! and call the feature services of the other crates. The decisions (the names, the files, the
//! checks, the seed table) are in [`crate::spec`].
//!
//! `demo e2e` is [`e2e`], with its subprocess steps in `net`; `demo seed` is [`seed`].

mod e2e;
mod net;

pub use e2e::e2e;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use std::time::Duration;

use serde_json::json;
use wg_analytics_feature::service::retrospective::retrospective;
use wg_automation_feature::service as automation;
use wg_config::set_path;
use wg_exec_feature::service::agents::LiveAgents;
use wg_exec_feature::service::attempts::{ListQuery, attempt_list};
use wg_exec_feature::service::run_flow::run_start;
use wg_exec_feature::spec::runs::StartRequest;
use wg_git_feature::service::task_commits;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{get, items, s};
use wg_intel_feature::service::embed::embed_task;
use wg_intel_feature::service::llm::Live;
use wg_json::Value;
use wg_registry_feature::service::{project_create, repo_depend};
use wg_registry_feature::service::{repo_register, workspace_create, workspace_list};
use wg_registry_feature::spec::registry::workspace_create_body;
use wg_registry_feature::spec::repos::RegisterInput;
use wg_runtime::{App, RequestContext, human_context};
use wg_runtime::{Feature, HandlerRegistry, Worker, WorkerOptions};
use wg_tasks_feature::service::acceptance::task_accept;
use wg_tasks_feature::service::{
    task_add_relation, task_comment, task_create, task_list, task_show, task_update,
};
use wg_types::parse_id;
use wg_verify_feature::service::api as verification;

use crate::spec::{self, REPOS, RepoSpec, written};

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

/// Where `demo seed` runs: the home, the features whose jobs the drain runs, and the progress
/// lines it collects (Python's `log`).
pub struct SeedEnv<'a> {
    pub home: Option<PathBuf>,
    pub features: &'static [&'static Feature],
    pub log: &'a mut dyn FnMut(&str),
}

/// `demo seed` (`seed_demo`): the preconditions of the command (exit 3 on a workspace that
/// exists, or on data without `--force`), then the workspace, the repositories, the projects, the
/// verification configs, the automation rule, the seven tasks and their runs, the embeddings and
/// the retrospective, each through the service the matching command calls.
///
/// # Errors
///
/// Returns the precondition failures, or the first service that fails.
pub fn seed(
    app: &mut App,
    env: &mut SeedEnv<'_>,
    workspace: &str,
    force: bool,
) -> Result<Value, CliFailure> {
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
    configure_demo(app, &spec::SEED_AGENT_MODES, true)?;
    (env.log)("configured heuristic inference and scripted demo agents");
    workspace_create(
        app,
        &context,
        &workspace_create_body(workspace, Some(&spec::title_case(workspace)), "DEMO", ""),
        true,
    )?;
    let data_dir = PathBuf::from(app.paths.data_dir().as_str());
    let repos_root = data_dir.join("demo-repos").join(workspace);
    let paths = create_repositories(&repos_root)?;
    for (_, path) in &paths {
        let input = RegisterInput {
            path: path.display().to_string(),
            ..RegisterInput::default()
        };
        repo_register(app, &context, Some(workspace), &input)?;
    }
    repo_depend(
        app,
        &context,
        Some(workspace),
        "web-client",
        "shared-sdk",
        "web client consumes the SDK",
        false,
    )?;
    for body in spec::seed_projects() {
        project_create(app, &context, Some(workspace), &body)?;
    }
    create_verification(app, &context, workspace)?;
    let rule = spec::seed_rule();
    automation::create(app, &context, Some(workspace), &rule)?;
    (env.log)("created repositories, projects, verification and automation");

    let keys = create_tasks(app, &context, workspace)?;
    drain(env)?;
    (env.log)("created tasks and classified them");

    for (position, (task, key)) in spec::SEED_TASKS.iter().zip(&keys).enumerate() {
        let request = StartRequest {
            agent: Some(task.agent.to_owned()),
            ..StartRequest::default()
        };
        let attempt = run_start(
            app,
            &context,
            &mut LiveAgents,
            Some(workspace),
            key,
            &request,
        )?;
        drain(env)?;
        if task.retry {
            let request = StartRequest {
                agent: Some("scripted".to_owned()),
                retry_of: Some(parse_id(&s(get(&attempt, "id")))?),
                ..StartRequest::default()
            };
            run_start(
                app,
                &context,
                &mut LiveAgents,
                Some(workspace),
                key,
                &request,
            )?;
            drain(env)?;
        }
        // The last two tasks (the failing agent and the research task) are left for review.
        if task_status(app, &context, workspace, key)? == "awaiting_review" && position < 5 {
            task_accept(app, &context, Some(workspace), key, "")?;
        }
        let status = task_status(app, &context, workspace, key)?;
        (env.log)(&format!("{key}: {status}"));
    }
    drain(env)?;
    for key in &keys {
        let shown = task_show(app, &context, Some(workspace), std::slice::from_ref(key))?;
        let Some(id) = shown.items.first().map(|item| s(get(item, "id"))) else {
            continue;
        };
        let embedded =
            parse_id(&id).and_then(|id| embed_task(app, &context, &mut Live, id, None, None));
        if let Err(error) = embedded {
            // Embeddings are optional enrichment.
            (env.log)(&format!("embedding skipped for {key}: {}", error.message));
        }
    }
    retrospective(
        app,
        &context,
        Some(workspace),
        &(14, None, None, true, None),
    )?;

    let mut commits = 0;
    for key in &keys {
        let linked = task_commits(app, &context, Some(workspace), key, 0.0)?;
        commits += items(Some(&linked)).len();
    }
    let tasks = task_list(app, &context, Some(workspace), &json!({}))?;
    let query = ListQuery {
        workspace: Some(workspace),
        task: None,
        statuses: &[],
        provider: None,
        limit: 1,
        offset: 0,
    };
    let attempts = attempt_list(app, &context, &query)?;
    let total = |page: &Value| s(get(page, "total")).parse::<i128>().unwrap_or(0);
    Ok(wg_json::dict(vec![
        ("workspace", Value::Str(workspace.into())),
        ("tasks", Value::Int(total(&tasks))),
        ("attempts", Value::Int(total(&attempts))),
        ("commits", Value::Int(i128::try_from(commits).unwrap_or(0))),
        (
            "repositories",
            Value::Str(repos_root.display().to_string().into()),
        ),
    ]))
}

fn task_status(
    app: &mut App,
    context: &RequestContext,
    workspace: &str,
    key: &str,
) -> Result<String, CliFailure> {
    let shown = task_show(app, context, Some(workspace), &[key.to_owned()])?;
    Ok(shown
        .items
        .first()
        .map(|item| s(get(item, "status")))
        .unwrap_or_default())
}

/// The verification configs of `seed_demo`: a contract check per repository and the optional
/// cross-repository integration check of the `platform` project.
fn create_verification(
    app: &mut App,
    context: &RequestContext,
    workspace: &str,
) -> Result<(), CliFailure> {
    let agent = demo_agent_path()?;
    for body in spec::seed_verification(&agent) {
        verification::create_config(app, context, workspace, &body)?;
    }
    Ok(())
}

/// The seven seed tasks, the umbrella task and the follow-up; the keys of the seven, in order.
fn create_tasks(
    app: &mut App,
    context: &RequestContext,
    workspace: &str,
) -> Result<Vec<String>, CliFailure> {
    let mut keys = Vec::new();
    for task in &spec::SEED_TASKS {
        let created = task_create(app, context, Some(workspace), &spec::task_body(task))?;
        keys.push(s(get(&created, "key")));
    }
    let parent = s(get(
        &task_create(app, context, Some(workspace), &spec::umbrella_body())?,
        "key",
    ));
    for key in &keys[..4] {
        task_update(
            app,
            context,
            Some(workspace),
            key,
            &json!({"parent": parent}),
        )?;
    }
    let follow = s(get(
        &task_create(
            app,
            context,
            Some(workspace),
            &spec::follow_body(&keys[0], &keys[1]),
        )?,
        "key",
    ));
    task_add_relation(
        app,
        context,
        Some(workspace),
        &follow,
        &json!({"target": keys[0], "relation_type": "follows_up"}),
    )?;
    task_comment(
        app,
        context,
        Some(workspace),
        &keys[0],
        &json!({"body": "Reproduced locally: token expiry at 60 min."}),
    )?;
    Ok(keys)
}

/// `_drain`: a worker with three threads runs the queue until nothing is runnable. The worker
/// opens its own application over the same home, as `worker run` does.
fn drain(env: &SeedEnv<'_>) -> Result<(), CliFailure> {
    drain_home(env.home.clone(), env.features)
}

/// [`drain`] over `home` with the jobs of `features`.
fn drain_home(
    home: Option<PathBuf>,
    features: &'static [&'static Feature],
) -> Result<(), CliFailure> {
    let options = WorkerOptions {
        home,
        concurrency: Some(3),
        poll_interval: Some(Duration::from_millis(50)),
        maintenance_interval: Some(Duration::from_millis(200)),
        ..WorkerOptions::default()
    };
    let mut worker = Worker::open(options, HandlerRegistry::from_features(features))?;
    worker.run_until_idle(Duration::from_secs(300))?;
    Ok(())
}

/// The demo's configuration (step 1): heuristic inference, the execution switch, the scripted
/// agents with the demo binary (DV4) and manual acceptance.
fn configure_demo(app: &mut App, agents: &[(&str, &str)], seeding: bool) -> Result<(), CliFailure> {
    let mut config = app.config()?;
    let mut settings: Vec<(&str, serde_json::Value)> = vec![
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
    ];
    if !seeding {
        settings.push(("verification.auto_accept", serde_json::json!(false)));
    }
    for (key, value) in settings {
        config = set_path(&config, key, value).ok_or_else(|| {
            CliFailure::fail(format!("cannot set {key}: not a table in config.toml"), 1)
        })?;
    }
    let executable = demo_agent_path()?;
    for (name, mode) in agents {
        let value = serde_json::json!({
            "type": "command",
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
