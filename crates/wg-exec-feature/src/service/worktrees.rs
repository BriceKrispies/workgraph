//! The worktree service's shell (WG-3773): crash recovery of Workgraph's managed worktrees, their
//! listing, and the release and removal of an attempt's worktrees, as `WorktreeManager` in
//! `integrations/git/worktrees.py` runs them.
//!
//! The decisions are the pure `spec::worktrees` module. This file reads the rows and the
//! attempts, tasks and stacks they belong to, checks the checkouts and the worktree directory on
//! disk, runs git in the checkouts, and applies each decision in one transaction.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::spec::worktrees::{
    Candidate, PrepareFacts, PrepareRequest, RecoveryInput, RecoveryReport, RecoveryRow,
    ReleaseTarget, Removal, WorktreeDraft, acquire_lease_decision, activated_decision,
    configured_isolation, git_failure, io_failure, orphaned_decision, path_key, plan_failure,
    prepare_decision, preparing_decision, recovery_decision, recovery_report_out, release_decision,
    stack_of, worktree_list_out,
};
use serde_json::json;
use wg_exec::manifest::{
    AttemptRef, ExecutionManifest, ManifestEntry, RepoFacts, RepoPlan, RepoRequest as ExecRequest,
    effective_target, plan_repo, worktree_dir_name,
};
use wg_exec::stack_base::{Sibling, SiblingChoice, choose_sibling, starts_from_integration_tip};
use wg_git::{Git, GitError, resolve};
pub use wg_git_feature::service::worktree_remove::{MANIFEST_FILE, worktree_remove};
use wg_git_feature::service::worktree_root::workspace_root;
use wg_git_feature::spec::worktree_root::{attempt_dir_name, taken_attempt_dir_name};
use wg_json::Value;
use wg_model::entity::{
    AgentAttempt, ManagedWorktree, Repository, ResourceLease, Task, VerificationConfig,
};
use wg_model::write::RowKey;
use wg_registry::checkout::primary_checkout;
use wg_runtime::App;
use wg_runtime::spec::RequestContext;
use wg_runtime::spec::effects::GIT_TIMEOUT_MS;
use wg_store::{Session, TaskWorld};
use wg_types::{AccessMode, Id, Permission, StackStatus, TaskStatus, WgError, WorktreeStatus};
use wg_verify::TaskScope;

/// The directory name under the worktrees root that holds stack worktrees.
const STACKS_DIR: &str = "stacks";

/// `WorktreeManager.recover`: reconciles the managed worktree rows with the disk and their
/// attempts, and reports the directories under the worktrees root that no row records.
/// `worker recover` runs it as the system (`RequestContext.system("cli")`).
pub fn worktree_recover(
    app: &mut App,
    context: &RequestContext,
) -> Result<RecoveryReport, WgError> {
    let root = PathBuf::from(app.paths.worktrees_dir().as_str());
    Ok(app
        .runtime
        .decide(
            context,
            |session| recovery_input(session, &root),
            |input, stamp| Ok(recovery_decision(input, stamp)),
        )?
        .reply)
}

/// `WorktreeManager.list`: the managed worktrees, newest first, of one workspace and status
/// when given. A status that is not a worktree status matches nothing.
pub fn worktree_list(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    status: Option<&str>,
) -> Result<Value, WgError> {
    app.runtime
        .stamps
        .stamp(context)?
        .require(&[Permission::RepoRead])?;
    // Python tests `if workspace:` and `if status:`, so an empty reference filters nothing.
    let workspace_id = match workspace.filter(|reference| !reference.is_empty()) {
        Some(reference) => Some(app.workspace_id(reference)?),
        None => None,
    };
    let status = match status.filter(|text| !text.is_empty()) {
        Some(text) => match WorktreeStatus::parse(text) {
            Some(status) => Some(status),
            None => return Ok(worktree_list_out(&[])),
        },
        None => None,
    };
    let rows = app
        .runtime
        .store
        .read(|session| session.worktrees_with_repository(workspace_id, status))?;
    Ok(worktree_list_out(&rows))
}

/// `WorktreeManager.release`: the attempt's leases are dropped and its worktrees are marked
/// released. With `remove`, each of them is then removed (see [`worktree_remove`]), and
/// the removals are returned.
pub fn worktree_release(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
    remove: bool,
    force: bool,
) -> Result<Vec<Removal>, WgError> {
    let holder = attempt_id.to_string();
    let targets: Vec<ReleaseTarget> = app
        .runtime
        .decide(
            context,
            |session| {
                let leases = session.leases_held_by(&holder)?;
                let rows = session.worktrees_of_attempt(
                    attempt_id,
                    &[
                        WorktreeStatus::Active,
                        WorktreeStatus::Preparing,
                        WorktreeStatus::Released,
                    ],
                )?;
                Ok((leases, rows))
            },
            |(leases, rows), _stamp| Ok(release_decision(leases, rows)),
        )?
        .reply;
    if !remove {
        return Ok(Vec::new());
    }
    targets
        .iter()
        .map(|target| worktree_remove(app, context, target.id, force))
        .collect()
}

/// The facts recovery decides from, read in the decision's transaction.
fn recovery_input(session: &mut Session<'_>, root: &Path) -> Result<RecoveryInput, WgError> {
    let stacks_root = resolve(&root.join(STACKS_DIR)).display().to_string();
    let open =
        session.worktrees_with_status(&[WorktreeStatus::Preparing, WorktreeStatus::Active])?;
    let mut rows = Vec::with_capacity(open.len());
    for row in open {
        let exists_on_disk = Path::new(&row.path).exists()
            && Git {
                path: PathBuf::from(&row.path),
                timeout_ms: GIT_TIMEOUT_MS,
            }
            .is_repository()
            .unwrap_or(false);
        let attempt = match row.attempt_id {
            Some(attempt_id) => session.attempt_by_id(attempt_id)?,
            None => None,
        };
        let task_closed = match attempt.as_ref() {
            Some(attempt) => session
                .fetch::<Task>(&RowKey::Task(attempt.task_id))?
                .is_some_and(|task| {
                    matches!(task.status, TaskStatus::Accepted | TaskStatus::Cancelled)
                        || task.archived_at.is_some()
                }),
            None => false,
        };
        let stack_ended = match stack_of(
            &resolve(Path::new(&row.path)).display().to_string(),
            &stacks_root,
        ) {
            Some(stack_dir) => match Id::parse_str(&stack_dir) {
                Ok(stack_id) => session.stack_by_id(stack_id)?.is_some_and(|stack| {
                    matches!(stack.status, StackStatus::Landed | StackStatus::Abandoned)
                }),
                Err(_) => false,
            },
            None => false,
        };
        rows.push(RecoveryRow {
            attempt_status: attempt.map(|attempt| attempt.status),
            row,
            exists_on_disk,
            task_closed,
            stack_ended,
        });
    }
    let known = session
        .worktrees_with_status(&[])?
        .iter()
        .map(|row| path_key(&resolve(Path::new(&row.path)).display().to_string()))
        .collect();
    Ok(RecoveryInput {
        rows,
        known,
        candidates: candidates(root),
    })
}

/// Every directory under each attempt directory of the worktrees root (the stacks directory
/// excluded), in name order. Python's `iterdir` order is the file system's; names are sorted here.
fn candidates(root: &Path) -> Vec<Candidate> {
    let mut found = Vec::new();
    for attempt_dir in sorted_children(root) {
        let is_stacks = attempt_dir
            .file_name()
            .is_some_and(|name| name == STACKS_DIR);
        if !attempt_dir.is_dir() || is_stacks {
            continue;
        }
        for child in sorted_children(&attempt_dir) {
            if child.is_dir() {
                found.push(Candidate {
                    key: path_key(&resolve(&child).display().to_string()),
                    path: child.display().to_string(),
                });
            }
        }
    }
    found
}

/// The entries of a directory, sorted by name; empty when it cannot be read.
fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries
}

/// The attempt and task `prepare` starts from, and the repositories it sets up.
struct PrepareStart {
    task: Task,
    /// The attempt's number under its task (`attempt.number`).
    number: i64,
    requests: Vec<PrepareRequest>,
}

/// What the final decision of `prepare` reads: the attempt, its task, the configurations that
/// apply and the task's scope.
struct PrepareWorld {
    attempt: AgentAttempt,
    task: Task,
    configs: Vec<VerificationConfig>,
    scope: TaskScope,
}

/// `WorktreeManager.prepare`: sets up the workspace of one attempt. Each repository gets its
/// own worktree (or the checkout itself, under `in_place`, behind an exclusive lease), the
/// baseline manifest, the change set and the verification checks are recorded, and the
/// execution manifest is written to the workspace directory. `requests` defaults to the task's
/// repository links, `isolation` to the configured one. A failure after the first repository
/// was set up releases and removes what was made.
pub fn worktree_prepare(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
    requests: Option<Vec<PrepareRequest>>,
    isolation: Option<&str>,
    expected_changes: &[String],
) -> Result<ExecutionManifest, WgError> {
    let isolation = match isolation {
        Some(text) => text.to_owned(),
        None => configured_isolation(&app.config()?),
    };
    let start = app
        .runtime
        .store
        .read(|session| prepare_start(session, attempt_id, requests, &isolation))?;
    let workspace_dir = match workspace_root(app, start.task.workspace_id)? {
        Some(root) => {
            let name = attempt_dir_name(&start.task.key.to_string(), start.number);
            let path = root.join(&name);
            if path.exists() {
                root.join(taken_attempt_dir_name(&name, attempt_id))
            } else {
                path
            }
        }
        None => PathBuf::from(app.paths.worktrees_dir().as_str()).join(attempt_id.to_string()),
    };
    fs::create_dir_all(&workspace_dir).map_err(|err| io_failure(&workspace_dir, &err))?;
    let mut entries = Vec::with_capacity(start.requests.len());
    for request in &start.requests {
        let planned = prepare_repo(
            app,
            context,
            &start,
            attempt_id,
            request,
            &isolation,
            &workspace_dir,
        );
        match planned {
            Ok(entry) => entries.push(entry),
            Err(err) => {
                // Python's `release(ctx, attempt_id, remove=True)` runs inside the `except`, so a
                // failure of the release itself propagates in place of the error that caused it.
                worktree_release(app, context, attempt_id, true, false)?;
                return Err(err);
            }
        }
    }
    let workspace_text = workspace_dir.display().to_string();
    let manifest = app
        .runtime
        .decide(
            context,
            |session| prepare_world(session, attempt_id),
            |world, stamp| {
                prepare_decision(
                    &PrepareFacts {
                        attempt: &world.attempt,
                        task: &world.task,
                        entries: &entries,
                        explicit_expected: expected_changes,
                        isolation: &isolation,
                        workspace_dir: &workspace_text,
                        configs: &world.configs,
                        scope: &world.scope,
                    },
                    stamp,
                )
            },
        )?
        .reply;
    let file = workspace_dir.join(MANIFEST_FILE);
    fs::write(&file, manifest.to_pretty_json()).map_err(|err| io_failure(&file, &err))?;
    Ok(manifest)
}

/// `WorktreeManager._prepare_repo`: plans one repository, then creates its lease (in place) or
/// its managed worktree, recording each step as it happens.
fn prepare_repo(
    app: &mut App,
    context: &RequestContext,
    start: &PrepareStart,
    attempt_id: Id,
    request: &PrepareRequest,
    isolation: &str,
    workspace_dir: &Path,
) -> Result<ManifestEntry, WgError> {
    let task = &start.task;
    let workspace_id = task.workspace_id;
    let (repo, checkout) = app.runtime.store.read(|session| {
        let repo = session
            .fetch::<Repository>(&RowKey::Repository(request.repository_id))?
            .filter(|repo| repo.workspace_id == workspace_id)
            .ok_or_else(|| {
                WgError::not_found(format!("repository {} not found", request.repository_id))
            })?;
        if request.access == "read_write" && repo.access_mode == AccessMode::ReadOnly {
            return Err(WgError::validation(format!(
                "repository {} is read-only",
                repo.name
            )));
        }
        let checkouts = session.checkouts(repo.id)?;
        let checkout = primary_checkout(&checkouts, true).cloned().ok_or_else(|| {
            WgError::git(format!(
                "repository {} has no available checkout",
                repo.name
            ))
        })?;
        Ok((repo, checkout))
    })?;
    let git = Git {
        path: PathBuf::from(&checkout.path),
        timeout_ms: GIT_TIMEOUT_MS,
    };
    let exec_request = ExecRequest {
        target_ref: request.target_ref.clone(),
        base: request.base.clone(),
    };
    let target = effective_target(&exec_request, repo.default_branch.as_deref());
    let resolved = match target {
        Some(name) => match git.resolve_commit(name) {
            Ok(sha) => Some(sha),
            Err(err @ GitError::InvalidRef(_)) => return Err(git_failure(err)),
            // The target does not resolve: the attempt starts from the checkout's HEAD.
            Err(_) => None,
        },
        None => None,
    };
    let head = git.head().map_err(git_failure)?;
    let current = match (isolation == "in_place", request.access == "read_write") {
        (true, true) => git.current_branch().map_err(git_failure)?,
        _ => None,
    };
    let repo_id = repo.id.to_string();
    let worktree_path = workspace_dir.join(worktree_dir_name(&repo.name));
    let worktree_text = worktree_path.display().to_string();
    let attempt_text = attempt_id.to_string();
    let task_key = task.key.to_string();
    let access_mode = repo.access_mode.as_str();
    let facts = RepoFacts {
        repository_id: &repo_id,
        repository: &repo.name,
        source_checkout_path: &checkout.path,
        access: &request.access,
        access_mode,
        isolation,
        request: exec_request.clone(),
        target,
        resolved_target: resolved.as_deref(),
        head: head.as_deref(),
        current_branch: current.as_deref(),
        worktree_path: &worktree_text,
        worktree_id: None,
    };
    let attempt_ref = AttemptRef {
        task_key: &task_key,
        attempt_number: start.number,
        attempt_id: &attempt_text,
    };
    match plan_repo(&facts, attempt_ref).map_err(plan_failure)? {
        RepoPlan::InPlace { entry } => {
            let key = format!("checkout:{}", checkout.id);
            let purpose = format!("in-place write {task_key}");
            app.runtime.decide(
                context,
                |session| session.lease_by_resource(&key),
                |existing: &Option<ResourceLease>, stamp| {
                    acquire_lease_decision(existing.as_ref(), &key, &attempt_text, &purpose, stamp)
                },
            )?;
            Ok(entry)
        }
        RepoPlan::Worktree { mut entry } => {
            let base = entry.sha.clone().unwrap_or_default();
            let draft = WorktreeDraft {
                workspace_id,
                repository_id: repo.id,
                source_checkout_id: checkout.id,
                attempt_id,
                path: &entry.checkout_path,
                branch: entry.branch.as_deref(),
                base_sha: &base,
            };
            let worktree_id = app
                .runtime
                .decide(
                    context,
                    |_| Ok(()),
                    |_, stamp| Ok(preparing_decision(&draft, stamp)),
                )?
                .reply;
            let added = match entry.branch.as_deref() {
                Some(branch) => git.worktree_add_branch(branch, &entry.checkout_path, &base),
                None => git.worktree_add_detached(&entry.checkout_path, &base),
            };
            if let Err(err) = added {
                let message = err.to_string();
                app.runtime.decide(
                    context,
                    |session| worktree_row(session, worktree_id),
                    |row, _| Ok(orphaned_decision(row, &message)),
                )?;
                return Err(git_failure(err));
            }
            let branch = entry.branch.clone();
            let base_info = entry.base.clone();
            app.runtime.decide(
                context,
                |session| worktree_row(session, worktree_id),
                |row, stamp| {
                    Ok(activated_decision(
                        row,
                        &repo.name,
                        branch.as_deref(),
                        base_info.clone(),
                        stamp,
                    ))
                },
            )?;
            entry.worktree_id = Some(worktree_id.to_string());
            Ok(entry)
        }
    }
}

/// The attempt's task and the requests `prepare` acts on. Outside `in_place`, a stack child's
/// request that chose no target gets the base its stack implies (`_stack_request`).
fn prepare_start(
    session: &mut Session<'_>,
    attempt_id: Id,
    requests: Option<Vec<PrepareRequest>>,
    isolation: &str,
) -> Result<PrepareStart, WgError> {
    let attempt = session
        .attempt_by_id(attempt_id)?
        .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))?;
    let task = session
        .fetch::<Task>(&RowKey::Task(attempt.task_id))?
        .ok_or_else(|| WgError::not_found(format!("task {} not found", attempt.task_id)))?;
    let requests = match requests {
        Some(requests) => requests,
        None => default_requests(session, &task)?,
    };
    let requests = if isolation == "in_place" {
        requests
    } else {
        requests
            .into_iter()
            .map(|request| stack_request(session, &task, request))
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(PrepareStart {
        task,
        number: attempt.number,
        requests,
    })
}

/// `WorktreeManager._stack_request`: gives a stack child's request the base its stack implies,
/// unless the caller chose a target.
fn stack_request(
    session: &mut Session<'_>,
    task: &Task,
    request: PrepareRequest,
) -> Result<PrepareRequest, WgError> {
    if request.target_ref.is_some() {
        return Ok(request);
    }
    let Some(repo) = session.fetch::<Repository>(&RowKey::Repository(request.repository_id))?
    else {
        return Ok(request);
    };
    Ok(match stack_base(session, task, &repo)? {
        Some((commit, base)) => PrepareRequest {
            target_ref: Some(commit),
            base: Some(base),
            ..request
        },
        None => request,
    })
}

/// `stack_base`: where a stack child's attempt starts in one repository, and why. `None` when
/// the task is not an item of an active stack that covers the repository.
fn stack_base(
    session: &mut Session<'_>,
    task: &Task,
    repo: &Repository,
) -> Result<Option<(String, serde_json::Value)>, WgError> {
    let Some(covering) = session.active_repository_for_task(task.id, repo.id)? else {
        return Ok(None);
    };
    let Some(item) = session.stack_item(covering.stack_id, task.id)? else {
        return Ok(None);
    };
    let no_checkout = || {
        WgError::validation(format!(
            "repository '{}' has no available checkout",
            repo.name
        ))
    };
    let checkouts = session.checkouts(repo.id)?;
    let git = || {
        primary_checkout(&checkouts, true).map(|checkout| Git {
            path: PathBuf::from(&checkout.path),
            timeout_ms: GIT_TIMEOUT_MS,
        })
    };
    let stack_id = covering.stack_id.to_string();
    if starts_from_integration_tip(item.state) {
        let git = git().ok_or_else(no_checkout)?;
        let tip = git
            .resolve_commit(&covering.integration_branch)
            .map_err(git_failure)?;
        return Ok(Some((
            tip.clone(),
            json!({
                "source": "integration_tip",
                "stack_id": stack_id,
                "stack_base_sha": covering.base_sha,
                "integration_branch": covering.integration_branch,
                "integration_tip": tip,
            }),
        )));
    }
    let mut siblings = Vec::new();
    for dependency in session.task_dependencies(task.id)? {
        if session
            .stack_item(covering.stack_id, dependency.depends_on_id)?
            .is_none()
        {
            continue;
        }
        if let Some(sibling) = session.fetch::<Task>(&RowKey::Task(dependency.depends_on_id))? {
            siblings.push(sibling);
        }
    }
    siblings.sort_by_key(|sibling| sibling.number);
    let mut heads = Vec::new();
    for sibling in &siblings {
        let head = session
            .latest_recorded_head(sibling.id, repo.id)?
            .map(|recorded| recorded.head_sha);
        heads.push(Sibling {
            key: sibling.key.to_string(),
            number: sibling.number,
            head,
        });
    }
    if heads.iter().all(|sibling| sibling.head.is_none()) {
        return Ok(Some((
            covering.base_sha.clone(),
            json!({
                "source": "stack_base",
                "stack_id": stack_id,
                "stack_base_sha": covering.base_sha,
            }),
        )));
    }
    let git = git().ok_or_else(no_checkout)?;
    let tips: Vec<&str> = heads
        .iter()
        .filter_map(|sibling| sibling.head.as_deref())
        .collect();
    let parents = commit_parents(&git, &tips)?;
    match choose_sibling(&heads, &parents) {
        SiblingChoice::StackBase => Ok(None),
        SiblingChoice::Diverged { first, second } => Err(WgError::validation(format!(
            "{} depends on {first} and {second}, whose changes to '{}' are not one line of history; add an explicit dependency between {first} and {second} so that one builds on the other",
            task.key, repo.name
        ))
        .with_detail("task", json!(task.key.to_string()))
        .with_detail("repository", json!(repo.name))
        .with_detail("siblings", json!([first, second]))),
        SiblingChoice::SiblingHead {
            sibling,
            sibling_head,
            superseded,
        } => Ok(Some((
            sibling_head.clone(),
            json!({
                "source": "sibling_head",
                "stack_id": stack_id,
                "stack_base_sha": covering.base_sha,
                "sibling": sibling,
                "sibling_head": sibling_head,
                "superseded": superseded
                    .iter()
                    .map(|other| json!({"task": other.task, "head": other.head}))
                    .collect::<Vec<_>>(),
            }),
        ))),
    }
}

/// The parents of every commit reachable from `tips` (`git rev-list --parents`).
fn commit_parents(git: &Git, tips: &[&str]) -> Result<BTreeMap<String, Vec<String>>, WgError> {
    let mut args = tips.to_vec();
    args.push("--");
    let mut full = vec!["--parents"];
    full.extend(args);
    let output = git.rev_list(&full).map_err(git_failure)?;
    Ok(output
        .stdout
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace().map(str::to_owned);
            let commit = words.next()?;
            Some((commit, words.collect()))
        })
        .collect())
}

/// `WorktreeManager.default_requests`: one request per repository link of the task, skipping the
/// archived or missing repositories; a read-only repository is requested read-only.
fn default_requests(
    session: &mut Session<'_>,
    task: &Task,
) -> Result<Vec<PrepareRequest>, WgError> {
    let world = session.task_world(Some(task.workspace_id), &[])?;
    let mut requests = Vec::new();
    for link in world.tasks.repositories_of(task.id) {
        let Some(repo) = session.fetch::<Repository>(&RowKey::Repository(link.repository_id))?
        else {
            continue;
        };
        if repo.archived_at.is_some() {
            continue;
        }
        let access = if repo.access_mode == AccessMode::ReadOnly {
            "read_only".to_owned()
        } else {
            link.access.clone()
        };
        requests.push(PrepareRequest {
            repository_id: repo.id,
            access,
            target_ref: None,
            base: None,
        });
    }
    Ok(requests)
}

/// What the final decision of `prepare` reads, in its transaction.
fn prepare_world(session: &mut Session<'_>, attempt_id: Id) -> Result<PrepareWorld, WgError> {
    let attempt = session
        .attempt_by_id(attempt_id)?
        .ok_or_else(|| WgError::not_found(format!("attempt {attempt_id} not found")))?;
    let task = session
        .fetch::<Task>(&RowKey::Task(attempt.task_id))?
        .ok_or_else(|| WgError::not_found(format!("task {} not found", attempt.task_id)))?;
    let world: TaskWorld = session.task_world(Some(task.workspace_id), &[])?;
    let scope = TaskScope {
        workspace_id: task.workspace_id,
        task_id: task.id,
        project_ids: world
            .tasks
            .projects_of(task.id)
            .map(|link| link.project_id)
            .collect(),
        repository_ids: world
            .tasks
            .repositories_of(task.id)
            .map(|link| link.repository_id)
            .collect(),
    };
    let configs = session.verification_configs(task.workspace_id)?;
    Ok(PrepareWorld {
        attempt,
        task,
        configs,
        scope,
    })
}

/// The managed worktree row `id`, as stored.
fn worktree_row(session: &mut Session<'_>, id: Id) -> Result<ManagedWorktree, WgError> {
    session
        .fetch::<ManagedWorktree>(&RowKey::ManagedWorktree(id))?
        .ok_or_else(|| WgError::not_found(format!("managed worktree {id} not found")))
}

/// The `worktrees` entry of `worker recover`'s report: the recovery pass as the system, printed
/// as the report of [`recovery_report_out`].
pub fn recovery_out(app: &mut App, context: &RequestContext) -> Result<Value, WgError> {
    Ok(recovery_report_out(&worktree_recover(app, context)?))
}

/// The `worktrees` entry of the queue's recovery report: the four path lists of the pass.
pub fn recovery_json(
    app: &mut App,
    context: &RequestContext,
) -> Result<serde_json::Value, WgError> {
    let report = worktree_recover(app, context)?;
    Ok(json!({
        "activated": report.activated,
        "released": report.released,
        "orphaned": report.orphaned,
        "unknown_dirs": report.unknown_dirs,
    }))
}
