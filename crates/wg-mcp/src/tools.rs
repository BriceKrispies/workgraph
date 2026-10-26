//! The workspace, project and repository tools (M01 to M08): `workspace_list`, `workspace_get`,
//! `project_list`, `project_get`, `repository_list`, `repository_get`, `repository_register` and
//! `repository_status`.
//!
//! Each tool resolves its references through `wg-registry` over a slice loaded from the store,
//! reads the rows its answer needs, and shapes them with the `wg-wire` DTOs. A registration is one
//! `wg-registry` decision applied through the runtime; the Git facts it needs are read by `wg-git`
//! before the decision. The answer is the `wg-json` value the DTOs build, converted once into the
//! structured output.

use std::path::{Path, PathBuf};

use serde_json::Value;
use wg_git::{Git, GitError};
use wg_json::Value as Dto;
use wg_mcpspec::{structured_output, text_argument};
use wg_model::entity::{Job, Project, Repository, RepositoryCheckout, RepositoryRemote};
use wg_model::{RegistrySlice, Row, RowKey, Write};
use wg_registry::boundary;
use wg_registry::checkout::{KIND_CLONE, KIND_WORKTREE, WorktreeRecord, primary_checkout};
use wg_registry::identity::{fingerprint, matched_by};
use wg_registry::register::{self, Facts, Matched, Request, WorktreeDetail, sync_key};
use wg_registry::{remote, resolve};
use wg_runtime::{App, agent_context};
use wg_runtimespec::effects::GIT_TIMEOUT_MS;
use wg_store::Session;
use wg_types::{ErrorCode, Id, WgError};
use wg_wire::dto::repository_status::{self, Status};
use wg_wire::dto::{
    mcp_shapes, project, project_summary, repository, task_brief, workspace, workspace_summary,
};

use crate::backend::Agent;

/// Answers tool `name` when this module owns it; `None` leaves the call to the backend's fallback.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let answer = match name {
        "workspace_list" => workspace_list(app),
        "workspace_get" => workspace_get(app, arguments),
        "project_list" => project_list(app, arguments),
        "project_get" => project_get(app, arguments),
        "repository_list" => repository_list(app, arguments),
        "repository_get" => repository_get(app, arguments),
        "repository_register" => repository_register(app, agent, arguments),
        "repository_status" => repository_status(app, arguments),
        _ => return None,
    };
    Some(answer.and_then(|dto| structured_output(&dto)))
}

/// The workspace a call names, or the default one when it names none.
fn workspace_of(app: &mut App, arguments: &Value) -> Result<Id, WgError> {
    match text_argument(arguments, "workspace") {
        Some(reference) => app.workspace_id(reference),
        None => app.default_workspace_id(),
    }
}

/// The rows that references in workspace `ws` resolve against: the workspace, its projects and
/// their memberships, and its repositories, archived ones included (as `resolve` needs them).
fn registry(session: &mut Session<'_>, ws: Id) -> Result<RegistrySlice, WgError> {
    let workspace_row = session
        .workspace_by_id(ws)?
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, format!("workspace {ws} not found")))?;
    let mut writes = vec![Write::insert(Row::Workspace(workspace_row))];
    for project_row in session.projects(ws, true)? {
        for link in session.project_repositories(project_row.id)? {
            writes.push(Write::insert(Row::ProjectRepository(link)));
        }
        writes.push(Write::insert(Row::Project(project_row)));
    }
    for repository_row in session.repositories(ws, None, true)? {
        writes.push(Write::insert(Row::Repository(repository_row)));
    }
    Ok(RegistrySlice::default().apply_all(&writes))
}

/// `ProjectOut` for `project`: its repository ids in the order the store lists them.
fn project_dto(session: &mut Session<'_>, project_row: &Project) -> Result<Dto, WgError> {
    let links = session.project_repositories(project_row.id)?;
    Ok(project::from_model(
        project_row,
        links.iter().map(|link| link.repository_id),
    ))
}

/// `RepositoryOut` for `repo`, with its remotes, checkouts, project memberships (from the slice)
/// and the repositories it depends on, both sorted.
fn repository_dto(
    session: &mut Session<'_>,
    registry: &RegistrySlice,
    repo: &Repository,
) -> Result<Dto, WgError> {
    let remotes = session.remotes(repo.id)?;
    let checkouts = session.checkouts(repo.id)?;
    let mut project_ids: Vec<Id> = registry
        .project_repositories
        .values()
        .filter(|link| link.repository_id == repo.id)
        .map(|link| link.project_id)
        .collect();
    project_ids.sort();
    let mut depends_on: Vec<Id> = session
        .dependencies_from(repo.id)?
        .iter()
        .map(|dependency| dependency.to_repository_id)
        .collect();
    depends_on.sort();
    Ok(repository::from_model(
        repo,
        &remotes,
        &checkouts,
        project_ids,
        depends_on,
    ))
}

fn workspace_list(app: &mut App) -> Result<Dto, WgError> {
    let rows = app.runtime.store.read(|session| session.workspaces())?;
    let items = rows
        .iter()
        .filter(|row| row.archived_at.is_none())
        .map(workspace::from_model)
        .collect();
    Ok(mcp_shapes::workspaces(items))
}

fn workspace_get(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    app.runtime.store.read(|session| {
        let row = session.workspace_by_id(ws)?.ok_or_else(|| {
            WgError::new(ErrorCode::NotFound, format!("workspace {ws} not found"))
        })?;
        let jobs = session.workspace_job_counts(ws)?;
        let job_count = |status: &str| {
            jobs.iter()
                .find(|(key, _)| key == status)
                .map_or(0, |(_, count)| *count)
        };
        let counts = workspace_summary::Counts {
            projects: i64::try_from(session.projects(ws, true)?.len()).unwrap_or(i64::MAX),
            repositories: i64::try_from(session.repositories(ws, None, false)?.len())
                .unwrap_or(i64::MAX),
            tasks_by_status: session.workspace_task_counts(ws)?,
            running_attempts: session.workspace_active_attempts(ws)?,
            queued_jobs: job_count("queued"),
            dead_jobs: job_count("dead"),
        };
        Ok(workspace_summary::from_model(&row, &counts))
    })
}

fn project_list(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let items = app.runtime.store.read(|session| {
        let mut items = Vec::new();
        for project_row in session.projects(ws, false)? {
            items.push(project_dto(session, &project_row)?);
        }
        Ok(items)
    })?;
    Ok(mcp_shapes::projects(items))
}

fn project_get(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let reference = text_argument(arguments, "project").unwrap_or_default();
    app.runtime.store.read(|session| {
        let registry = registry(session, ws)?;
        let project_row = resolve::project(&registry, ws, reference)?.clone();
        let repositories = session.repositories(ws, Some(project_row.id), true)?;
        let tasks_by_status = session.project_task_counts(project_row.id)?;
        let mut recent = Vec::new();
        for task_row in session.project_recent_tasks(project_row.id, RECENT_TASKS)? {
            recent.push(task_brief::from_model(&task_row));
        }
        let project_value = project_dto(session, &project_row)?;
        Ok(project_summary::from_model(
            project_value,
            &repositories,
            &tasks_by_status,
            recent,
        ))
    })
}

fn repository_list(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let project_ref = text_argument(arguments, "project");
    let items = app.runtime.store.read(|session| {
        let registry = registry(session, ws)?;
        let project_id = match project_ref {
            Some(reference) => Some(resolve::project(&registry, ws, reference)?.id),
            None => None,
        };
        let mut items = Vec::new();
        for repo in session.repositories(ws, project_id, false)? {
            items.push(repository_dto(session, &registry, &repo)?);
        }
        Ok(items)
    })?;
    Ok(mcp_shapes::repositories(items))
}

fn repository_get(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let reference = text_argument(arguments, "repository").unwrap_or_default();
    app.runtime.store.read(|session| {
        let registry = registry(session, ws)?;
        let repo = resolve::repository(&registry, ws, reference, true)?.clone();
        repository_dto(session, &registry, &repo)
    })
}

/// `repository_register`: `RepositoryService.register`. The Git facts are read first (outside any
/// transaction), then the decision runs in one write transaction with the lookups it needs.
fn repository_register(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let raw = text_argument(arguments, "path").unwrap_or_default();
    let raw_path = Path::new(raw);
    if !raw_path.is_dir() {
        return Err(validation(format!(
            "path {raw} does not exist or is not a directory"
        )));
    }
    let probe = Git {
        path: raw_path.to_path_buf(),
        timeout_ms: GIT_TIMEOUT_MS,
    };
    if !probe.is_repository().map_err(git_failure)? {
        return Err(validation(format!("{raw} is not a Git repository")));
    }
    let top = probe.toplevel().map_err(git_failure)?;
    let top_path = top.display().to_string();
    let git = Git {
        path: top.clone(),
        timeout_ms: GIT_TIMEOUT_MS,
    };
    let root_commits = git.root_commits().map_err(git_failure)?;
    let remotes = git.remotes().map_err(git_failure)?;
    let is_worktree = git.is_worktree().map_err(git_failure)?;
    let default_branch = git.default_branch().map_err(git_failure)?;
    let shallow = git.is_shallow().map_err(git_failure)?;
    let entries = git.worktrees().map_err(git_failure)?;
    let worktrees = entries
        .iter()
        .map(|entry| {
            (
                WorktreeRecord {
                    path: resolved(&entry.path),
                    bare: entry.bare,
                    prunable: entry.prunable,
                },
                WorktreeDetail {
                    branch: entry.branch.clone(),
                    detached: entry.detached,
                    locked: entry.locked,
                },
            )
        })
        .collect();
    let dir_name = top
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let allowed_roots = app.runtime.store.read(|session| {
        let workspace_row = session.workspace_by_id(ws)?.ok_or_else(|| {
            WgError::new(ErrorCode::NotFound, format!("workspace {ws} not found"))
        })?;
        Ok(boundary::allowed_roots(&workspace_row.settings))
    })?;
    let managed = [
        app.paths.worktrees_dir(),
        app.paths.runs_dir(),
        app.paths.artifacts_dir(),
    ]
    .into_iter()
    .map(|root| root.as_str().to_owned())
    .collect();
    let request = Request {
        workspace_id: ws,
        path: top_path,
        dir_name,
        requested_name: text_argument(arguments, "name").map(str::to_owned),
        read_only: arguments
            .get("read_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        fingerprint: fingerprint(&root_commits),
        remotes,
        kind: if is_worktree {
            KIND_WORKTREE
        } else {
            KIND_CLONE
        },
        default_branch,
        shallow,
        worktrees,
        allowed_roots,
        managed_roots: managed,
    };
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    let outcome = app.runtime.decide(
        &context,
        |session| register_facts(session, &request),
        |facts, stamp| register::register(&request, facts, stamp),
    )?;
    let id = outcome.reply;
    app.runtime.store.read(|session| {
        let repo = session
            .fetch::<Repository>(&RowKey::Repository(id))?
            .ok_or_else(|| {
                WgError::new(ErrorCode::NotFound, format!("repository {id} not found"))
            })?;
        let registry = registry(session, ws)?;
        repository_dto(session, &registry, &repo)
    })
}

/// `repository_status`: the working tree of the repository's primary checkout (`RepositoryService.status`).
fn repository_status(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let reference = text_argument(arguments, "repository").unwrap_or_default();
    let (repo, checkouts) = app.runtime.store.read(|session| {
        let registry = registry(session, ws)?;
        let repo = resolve::repository(&registry, ws, reference, true)?.clone();
        let checkouts = session.checkouts(repo.id)?;
        Ok((repo, checkouts))
    })?;
    let status = match primary_checkout(&checkouts, true) {
        None => Status {
            error: Some("no available checkout".to_owned()),
            ..Status::default()
        },
        Some(checkout) => {
            let git = Git {
                path: PathBuf::from(&checkout.path),
                timeout_ms: GIT_TIMEOUT_MS,
            };
            match git.status_porcelain() {
                Err(err) => Status {
                    checkout_path: Some(checkout.path.clone()),
                    error: Some(err.to_string()),
                    ..Status::default()
                },
                Ok(info) => Status {
                    checkout_path: Some(checkout.path.clone()),
                    available: true,
                    detached: info.branch.is_none(),
                    branch: info.branch,
                    head: info.head,
                    staged: info.staged,
                    unstaged: info.unstaged,
                    untracked: info.untracked,
                    ahead: info.ahead,
                    behind: info.behind,
                    is_shallow: git.is_shallow().map_err(git_failure)?,
                    error: None,
                },
            }
        }
    };
    Ok(repository_status::from_model(repo.id, &repo.name, &status))
}

/// The facts a registration reads, in the decision's transaction: the repositories the request
/// may match (by path, fingerprint and remote), the names taken, and the queued sync job.
fn register_facts(session: &mut Session<'_>, request: &Request) -> Result<Facts, WgError> {
    let ws = request.workspace_id;
    let by_path = session
        .repository_by_checkout_path(ws, &request.path)?
        .into_iter()
        .next();
    let by_path = match by_path {
        Some(repo) => Some(matched(session, repo)?),
        None => None,
    };
    let by_fingerprint = match &request.fingerprint {
        Some(value) => session
            .repository_by_fingerprint(ws, value)?
            .into_iter()
            .next(),
        None => None,
    };
    let by_fingerprint = match by_fingerprint {
        Some(repo) => Some(matched(session, repo)?),
        None => None,
    };
    let urls: Vec<String> = request
        .remotes
        .iter()
        .map(|(_, url)| remote::normalize_remote_url(url))
        .collect();
    let by_remote = session
        .repository_by_remote_urls(ws, &urls)?
        .into_iter()
        .next();
    let by_remote = match by_remote {
        Some(repo) => Some(matched(session, repo)?),
        None => None,
    };
    let taken_names = session
        .repositories(ws, None, true)?
        .into_iter()
        .map(|repo| repo.name)
        .collect();
    let chosen = matched_by(
        by_path.as_ref().map(|m| m.repository.id),
        by_fingerprint.as_ref().map(|m| m.repository.id),
        by_remote.as_ref().map(|m| m.repository.id),
    );
    let sync_job: Option<Job> = match chosen {
        Some((id, _)) => session.job_by_idempotency_key(&sync_key(id, &request.path))?,
        None => None,
    };
    Ok(Facts {
        by_path,
        by_fingerprint,
        by_remote,
        taken_names,
        sync_job,
    })
}

/// A repository with its checkouts and remotes, as a lookup needs them.
fn matched(session: &mut Session<'_>, repository: Repository) -> Result<Matched, WgError> {
    let checkouts: Vec<RepositoryCheckout> = session.checkouts(repository.id)?;
    let remotes: Vec<RepositoryRemote> = session.remotes(repository.id)?;
    Ok(Matched {
        repository,
        checkouts,
        remotes,
    })
}

/// A path as `Path.resolve()` writes it: canonical, without the Windows verbatim prefix. A path
/// that does not exist (a prunable worktree) is kept as git reports it.
fn resolved(path: &str) -> String {
    match std::fs::canonicalize(path) {
        Ok(canonical) => canonical
            .display()
            .to_string()
            .trim_start_matches(r"\\?\")
            .to_owned(),
        Err(_) => path.to_owned(),
    }
}

fn validation(message: String) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}

fn git_failure(err: GitError) -> WgError {
    WgError::new(ErrorCode::WorkgraphError, err.to_string())
}

/// Recent tasks a project summary lists (`ProjectService.summary` takes ten).
const RECENT_TASKS: i64 = 10;
