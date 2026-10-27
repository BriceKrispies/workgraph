//! The repository entry points of the application (IF-04): the services behind `repo register`,
//! `list`, `show`, `refresh`, `status`, `update`, `remove`, `remove-checkout` and `depend`.
//!
//! Each one checks the permission, resolves the workspace, loads the rows its decision reads in
//! the transaction of the call, hands them to the decision of `wg_registry` and reads the
//! repository back for the answer (`RepositoryOut`). Git is read before the transaction opens, as
//! the Python service does.

use std::path::{Path, PathBuf};

use crate::spec::dto::repository;
use crate::spec::dto::repository_status::{self, Status};
use crate::spec::repos::{GitFacts, RegisterInput, git_failure, register_request};
use wg_git::Git;
use wg_json::Value;
use wg_model::entity::{Job, Repository, RepositoryCheckout, RepositoryRemote};
use wg_model::{RegistrySlice, RowKey};
use wg_registry::admin::{self, Patch};
use wg_registry::checkout::{WorktreeDetail, WorktreeRecord, primary_checkout};
use wg_registry::identity::{fingerprint, matched_by};
use wg_registry::register::{self, Facts, Matched, Request, sync_key};
use wg_registry::{boundary, refresh, remote, resolve};
use wg_runtimespec::RequestContext;
use wg_runtimespec::effects::GIT_TIMEOUT_MS;
use wg_store::Session;
use wg_types::{Id, Permission, WgError};

use crate::service::reads;
use wg_runtime::App;

/// A git handle on `$path` with the default timeout. A macro, not a function: a function this
/// small would count as shell logic that could be pure.
macro_rules! git_at {
    ($path:expr) => {
        Git {
            path: Path::new(&$path).to_path_buf(),
            timeout_ms: GIT_TIMEOUT_MS,
        }
    };
}

/// A path as `Path.resolve()` writes it: canonical, without the Windows verbatim prefix. A path
/// that does not exist (a prunable worktree) is made absolute and kept as given.
fn resolved(path: &str) -> String {
    let canonical = std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .map_or_else(|_| path.to_owned(), |found| found.display().to_string());
    canonical.trim_start_matches(r"\\?\").to_owned()
}

/// `Path.expanduser()`: a leading `~` is the home directory.
fn expand_user(path: &str) -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var("USERPROFILE")
    } else {
        std::env::var("HOME")
    };
    match (path.strip_prefix('~'), home) {
        (Some(rest), Ok(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            PathBuf::from(format!("{home}{rest}"))
        }
        _ => PathBuf::from(path),
    }
}

/// `repository_out`: the repository with its remotes, checkouts (oldest first), project ids and
/// the repositories it depends on (both sorted).
pub fn repository_out(
    session: &mut Session<'_>,
    registry: &RegistrySlice,
    repo: &Repository,
) -> Result<Value, WgError> {
    let remotes = reads::remotes(session, repo.id)?;
    let checkouts = reads::checkouts_by_creation(session, repo.id)?;
    let mut project_ids: Vec<Id> = registry
        .project_repositories
        .values()
        .filter(|link| link.repository_id == repo.id)
        .map(|link| link.project_id)
        .collect();
    project_ids.sort();
    let mut depends_on: Vec<Id> = reads::dependencies_from(session, repo.id)?
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

/// A repository with its checkouts (oldest first) and remotes, as a decision reads them.
fn matched(session: &mut Session<'_>, repository: Repository) -> Result<Matched, WgError> {
    let checkouts: Vec<RepositoryCheckout> = reads::checkouts_by_creation(session, repository.id)?;
    let remotes: Vec<RepositoryRemote> = reads::remotes(session, repository.id)?;
    Ok(Matched {
        repository,
        checkouts,
        remotes,
    })
}

/// The facts a registration reads, in the decision's transaction: the repositories the request
/// may match (by path, fingerprint and remote), the names taken, the queued sync job and the
/// registry the project references resolve in.
fn register_facts(session: &mut Session<'_>, request: &Request) -> Result<Facts, WgError> {
    let ws = request.workspace_id;
    let by_path = reads::repository_by_checkout_path(session, ws, &request.path)?
        .into_iter()
        .next();
    let by_path = by_path.map(|repo| matched(session, repo)).transpose()?;
    let by_fingerprint = match &request.fingerprint {
        Some(value) => reads::repository_by_fingerprint(session, ws, value)?
            .into_iter()
            .next(),
        None => None,
    };
    let by_fingerprint = by_fingerprint
        .map(|repo| matched(session, repo))
        .transpose()?;
    let urls: Vec<String> = request
        .remotes
        .iter()
        .map(|(_, url)| remote::normalize_remote_url(url))
        .collect();
    let by_remote = reads::repository_by_remote_urls(session, ws, &urls)?
        .into_iter()
        .next();
    let by_remote = by_remote.map(|repo| matched(session, repo)).transpose()?;
    let registry = reads::registry_slice(session, ws)?;
    let taken_names = registry
        .repositories
        .values()
        .map(|repo| repo.name.clone())
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
        registry,
    })
}

/// The worktrees git lists for the repository at `path`, with their details.
fn worktree_list(path: &str) -> Result<Vec<(WorktreeRecord, WorktreeDetail)>, WgError> {
    let entries = git_at!(path)
        .worktrees()
        .map_err(|err| git_failure(err.to_string()))?;
    Ok(entries
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
        .collect())
}

/// A permission check against a fresh stamp.
fn require_repo(
    app: &mut App,
    context: &RequestContext,
    permission: Permission,
) -> Result<(), WgError> {
    app.runtime.stamps.stamp(context)?.require(&[permission])
}

/// The roots a path must lie under (when set) and the directories it must stay out of.
fn boundary_roots(app: &mut App, workspace_id: Id) -> Result<(Vec<String>, Vec<String>), WgError> {
    let allowed = app.runtime.store.read(|session| {
        let row = session
            .workspace_by_id(workspace_id)?
            .ok_or_else(|| WgError::not_found(format!("workspace {workspace_id} not found")))?;
        Ok(boundary::allowed_roots(&row.settings))
    })?;
    let managed = [
        app.paths.worktrees_dir(),
        app.paths.runs_dir(),
        app.paths.artifacts_dir(),
    ]
    .into_iter()
    .map(|root| root.as_str().to_owned())
    .collect();
    Ok((allowed, managed))
}

/// `RepositoryOut` of a repository row as it was just written, so its `health_detail` keeps the key
/// order the decision built (the store reload would sort it).
fn repository_row_value(
    app: &mut App,
    workspace_id: Id,
    repo: &Repository,
) -> Result<Value, WgError> {
    app.runtime.store.read(|session| {
        let registry = reads::registry_slice(session, workspace_id)?;
        repository_out(session, &registry, repo)
    })
}

/// `RepositoryOut` of repository `id`.
fn repository_value(app: &mut App, workspace_id: Id, id: Id) -> Result<Value, WgError> {
    app.runtime.store.read(|session| {
        let registry = reads::registry_slice(session, workspace_id)?;
        let repo = session
            .fetch::<Repository>(&RowKey::Repository(id))?
            .ok_or_else(|| WgError::not_found(format!("repository {id} not found")))?;
        repository_out(session, &registry, &repo)
    })
}

/// `RepositoryService.register`: registers the repository at a path, or adds the path as
/// another checkout of a known one.
pub fn repo_register(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    input: &RegisterInput,
) -> Result<Value, WgError> {
    require_repo(app, context, Permission::RepoRegister)?;
    let raw = expand_user(&input.path);
    if !raw.is_dir() {
        return Err(WgError::validation(format!(
            "path {} does not exist or is not a directory",
            raw.display()
        )));
    }
    let probe = git_at!(raw);
    if !probe
        .is_repository()
        .map_err(|err| git_failure(err.to_string()))?
    {
        return Err(WgError::validation(format!(
            "{} is not a Git repository",
            raw.display()
        )));
    }
    let top = probe
        .toplevel()
        .map_err(|err| git_failure(err.to_string()))?;
    let git = git_at!(top);
    let facts = GitFacts {
        top: top.display().to_string(),
        dir_name: top
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        root_commits: git
            .root_commits()
            .map_err(|err| git_failure(err.to_string()))?,
        remotes: git.remotes().map_err(|err| git_failure(err.to_string()))?,
        is_worktree: git
            .is_worktree()
            .map_err(|err| git_failure(err.to_string()))?,
        default_branch: git
            .default_branch()
            .map_err(|err| git_failure(err.to_string()))?,
        shallow: git
            .is_shallow()
            .map_err(|err| git_failure(err.to_string()))?,
        worktrees: worktree_list(&top.display().to_string())?,
    };
    let scope = app.workspace_or_default(workspace)?;
    let (allowed, managed) = boundary_roots(app, scope)?;
    let request = register_request(scope, input, facts, allowed, managed);
    let id = app
        .runtime
        .decide(
            context,
            |session| register_facts(session, &request),
            |facts, stamp| register::register(&request, facts, stamp),
        )?
        .reply;
    repository_value(app, scope, id)
}

/// `RepositoryService.list`: the repositories of the workspace by name; archived ones only
/// with `include_archived`, and `project` narrows to its members.
pub fn repo_list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    include_archived: bool,
    project: Option<&str>,
) -> Result<Vec<Value>, WgError> {
    require_repo(app, context, Permission::RepoRead)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.store.read(|session| {
        let registry = reads::registry_slice(session, scope)?;
        let project_id = project
            .map(|reference| resolve::project(&registry, scope, reference).map(|p| p.id))
            .transpose()?;
        session
            .repositories(scope, project_id, include_archived)?
            .iter()
            .map(|repo| repository_out(session, &registry, repo))
            .collect()
    })
}

/// `RepositoryService.get`: one repository, archived or not.
pub fn repo_get(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    require_repo(app, context, Permission::RepoRead)?;
    let scope = app.workspace_or_default(workspace)?;
    app.runtime.store.read(|session| {
        let registry = reads::registry_slice(session, scope)?;
        let repo = resolve::repository(&registry, scope, reference, true)?.clone();
        repository_out(session, &registry, &repo)
    })
}

/// `RepositoryService.refresh`: re-checks the checkouts, remotes and health against git.
pub fn repo_refresh(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    require_repo(app, context, Permission::RepoRead)?;
    let scope = app.workspace_or_default(workspace)?;
    let known = app.runtime.store.read(|session| {
        let registry = reads::registry_slice(session, scope)?;
        let repo = resolve::repository(&registry, scope, reference, true)?.clone();
        matched(session, repo)
    })?;
    // What git says about each checkout path.
    let probes = known
        .checkouts
        .iter()
        .map(|checkout| {
            let git = git_at!(checkout.path);
            if !git
                .is_repository()
                .map_err(|err| git_failure(err.to_string()))?
            {
                return Ok(refresh::Probe::default());
            }
            Ok(refresh::Probe {
                is_repository: true,
                fingerprint: fingerprint(
                    &git.root_commits()
                        .map_err(|err| git_failure(err.to_string()))?,
                ),
                remotes: git.remotes().map_err(|err| git_failure(err.to_string()))?,
                default_branch: git
                    .default_branch()
                    .map_err(|err| git_failure(err.to_string()))?,
                shallow: git
                    .is_shallow()
                    .map_err(|err| git_failure(err.to_string()))?,
            })
        })
        .collect::<Result<Vec<_>, WgError>>()?;
    let live = refresh::first_live(&refresh::observations(&known, &probes))
        .and_then(|index| known.checkouts.get(index));
    let worktrees = live
        .map(|checkout| worktree_list(&checkout.path))
        .transpose()?
        .unwrap_or_default();
    let (allowed_roots, managed_roots) = boundary_roots(app, scope)?;
    let request = refresh::Request {
        probes,
        worktrees,
        allowed_roots,
        managed_roots,
    };
    let row = app
        .runtime
        .decide(
            context,
            |session| {
                let registry = reads::registry_slice(session, scope)?;
                let repo = resolve::repository(&registry, scope, reference, true)?.clone();
                matched(session, repo)
            },
            |loaded, stamp| refresh::refresh(&request, loaded, stamp),
        )?
        .reply;
    repository_row_value(app, scope, &row)
}

/// `RepositoryService.status`: the working tree of the primary checkout.
pub fn repo_status(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    require_repo(app, context, Permission::RepoRead)?;
    let scope = app.workspace_or_default(workspace)?;
    let found = app.runtime.store.read(|session| {
        let registry = reads::registry_slice(session, scope)?;
        let repo = resolve::repository(&registry, scope, reference, true)?.clone();
        matched(session, repo)
    })?;
    let status = match primary_checkout(&found.checkouts, true) {
        None => Status {
            error: Some("no available checkout".to_owned()),
            ..Status::default()
        },
        Some(checkout) => {
            let git = git_at!(checkout.path);
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
                    is_shallow: git
                        .is_shallow()
                        .map_err(|err| git_failure(err.to_string()))?,
                    error: None,
                },
            }
        }
    };
    Ok(repository_status::from_model(
        found.repository.id,
        &found.repository.name,
        &status,
    ))
}

/// `RepositoryService.update`: changes the fields the patch sets.
pub fn repo_update(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    patch: &Patch,
) -> Result<Value, WgError> {
    require_repo(app, context, Permission::RepoAdmin)?;
    let scope = app.workspace_or_default(workspace)?;
    let id = app
        .runtime
        .decide(
            context,
            |session| {
                let registry = reads::registry_slice(session, scope)?;
                let repo = resolve::repository(&registry, scope, reference, false)?.clone();
                let names: Vec<String> = registry
                    .repositories
                    .values()
                    .map(|other| other.name.clone())
                    .collect();
                Ok((repo, names))
            },
            |(repo, names), stamp| admin::update(stamp, repo, names, patch),
        )?
        .reply;
    repository_value(app, scope, id)
}

/// `RepositoryService.remove`: archives the registration; files on disk are untouched.
pub fn repo_remove(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    require_repo(app, context, Permission::RepoAdmin)?;
    let scope = app.workspace_or_default(workspace)?;
    let id = app
        .runtime
        .decide(
            context,
            |session| {
                let registry = reads::registry_slice(session, scope)?;
                Ok(resolve::repository(&registry, scope, reference, false)?.clone())
            },
            |repo, stamp| admin::remove(stamp, repo),
        )?
        .reply;
    repository_value(app, scope, id)
}

/// `RepositoryService.remove_checkout`: forgets one checkout path of a repository.
pub fn repo_remove_checkout(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    path: &str,
) -> Result<Value, WgError> {
    require_repo(app, context, Permission::RepoAdmin)?;
    let scope = app.workspace_or_default(workspace)?;
    let absolute = resolved(&expand_user(path).display().to_string());
    let id = app
        .runtime
        .decide(
            context,
            |session| {
                let registry = reads::registry_slice(session, scope)?;
                let repo = resolve::repository(&registry, scope, reference, true)?.clone();
                matched(session, repo)
            },
            |loaded, stamp| admin::remove_checkout(stamp, loaded, path, &absolute),
        )?
        .reply;
    repository_value(app, scope, id)
}

/// `RepositoryService.add_dependency` and `remove_dependency`: declares, or with `remove`
/// drops, that `reference` depends on `depends_on`.
pub fn repo_depend(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    depends_on: &str,
    description: &str,
    remove: bool,
) -> Result<Value, WgError> {
    require_repo(app, context, Permission::RepoAdmin)?;
    let scope = app.workspace_or_default(workspace)?;
    let id = app
        .runtime
        .decide(
            context,
            |session| {
                let registry = reads::registry_slice(session, scope)?;
                let repo = resolve::repository(&registry, scope, reference, false)?.clone();
                let other = resolve::repository(&registry, scope, depends_on, remove)?.clone();
                let existing = reads::dependency_between(session, repo.id, other.id)?;
                Ok((repo, other, existing))
            },
            |(repo, other, existing), stamp| {
                if remove {
                    admin::remove_dependency(stamp, repo, existing.as_ref())
                } else {
                    admin::add_dependency(stamp, repo, other, existing.as_ref(), description)
                }
            },
        )?
        .reply;
    repository_value(app, scope, id)
}
