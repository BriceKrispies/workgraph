//! The git entry points of the application (IF-12): the services behind `git log`, `show`, `link`,
//! `unlink`, `changes`, `diff`, `surface`, `analyze`, `worktrees`, `cleanup-worktrees`, `install-hook`
//! and `sync` (the pipeline itself is `git_sync`).
//!
//! Each one resolves the workspace and the repository or task, reads the rows it needs, runs the
//! git calls it needs outside any transaction, and then decides in one transaction with the pure
//! functions of `wg_runtime::spec::git`. Ported from `integrations/git/service.py` and
//! `integrations/git/worktrees.py`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::store;
use super::sync::{ingest_batch, repo_state};
use super::worktree_remove::remove_empty_attempt_dir;
use super::worktree_root::{configured_roots, in_attempt_dir};
use crate::spec::analysis::{self, FileResult, manifest_shas};
use crate::spec::links::{self, ExplicitLinkIds};
use crate::spec::reads::{
    self, ChangeCommit, ChangesInput, DetailInput, LogItem, TaskRef, commit_ids as ids_of,
    group_by_commit, missing_gap_targets, short_sha, to_i64, unique_commit_ids,
};
use crate::spec::sql::TaskCommitLink as TaskCommitLinkRow;
use crate::spec::surface::{self, FileInput};
use crate::spec::worktrees::{self, WorktreeRow};
use crate::spec::worktrees::{is_within, removal_result};
use wg_codeanalysis::{analyze_file, parse_hunks};
use wg_codeparse::extract_symbols;
use wg_git::{Git, RunOptions};
use wg_gitcore::{
    EMPTY_TREE, FileChange, SurfacePlan, SurfaceRecord, SurfaceRepo, parse_numstat_z, plan_surface,
};
use wg_iface::cli::CliFailure;
use wg_json::{Value, dict};
use wg_model::entity::git::{Commit, CommitTaskLink, GitObjectGap, TaskChangeSurface};
use wg_model::entity::repository::Repository;
use wg_model::entity::run::ManagedWorktree;
use wg_model::event::{EventPayload, TaskChangeSurfaceComputed, TaskCommitLinked};
use wg_model::{Decision, EventDraft, Row, RowKey, Write};
use wg_registry::resolve;
use wg_runtime::spec::RequestContext;
use wg_store::Session;
use wg_types::{Id, Permission, WgError, WorktreeStatus};
use wg_wire::convention::{int, text};
use wg_wire::dto::task_diff::{DiffPart, Missing};

use crate::spec::git_error as git_err;
use wg_runtime::App;
use wg_types::pyval::prefix;

/// The default `git diff` budget in characters (`max_bytes=400_000`).
pub const DIFF_MAX_BYTES: usize = 400_000;

/// Options of a git call whose non-zero exit is an outcome, not an error (`check=False`).
pub(crate) const NO_CHECK_OPTIONS: RunOptions<'static> = RunOptions {
    check: false,
    stdin: None,
    timeout_ms: None,
    env: &[],
};

/// The git-side identity of a repository the entry points read.
#[derive(Debug, Clone)]
pub(crate) struct RepoRow {
    pub(crate) repository: Repository,
}

/// A permission check against a fresh stamp.
fn git_require(
    app: &mut App,
    context: &RequestContext,
    permission: Permission,
) -> Result<(), WgError> {
    app.runtime.stamps.stamp(context)?.require(&[permission])
}

/// The repository `reference` names in the workspace, with its primary checkout's path.
pub(crate) fn repository_row(
    app: &mut App,
    workspace_id: Id,
    reference: &str,
    include_archived: bool,
) -> Result<RepoRow, WgError> {
    app.runtime.store.read(|session| {
        let slice = session.registry_world(Some(workspace_id))?;
        let repository =
            resolve::repository(&slice, workspace_id, reference, include_archived)?.clone();
        Ok(RepoRow { repository })
    })
}

/// The task `reference` names, in the workspace when one is given, else in any workspace.
fn task_row(
    app: &mut App,
    workspace_id: Option<Id>,
    reference: &str,
) -> Result<wg_model::entity::task::Task, WgError> {
    app.runtime.store.read(|session| {
        let world = session.task_world(workspace_id, &[])?;
        Ok(resolve::task(&world.tasks, workspace_id, reference)?.clone())
    })
}

/// `GitService.repository_commits` (`git log`): one page of the repository's commits, newest
/// first, from `offset`; unreachable commits only with `include_unreachable`.
#[allow(clippy::too_many_arguments)]
pub fn git_log(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    repository: &str,
    limit: i64,
    linked_only: bool,
    offset: i64,
    include_unreachable: bool,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::GitRead)?;
    let workspace_id = app.workspace_or_default(workspace)?;
    let repo = repository_row(app, workspace_id, repository, true)?;
    let repository_id = repo.repository.id;
    let reachable_only = !include_unreachable;
    let (total, commits, links) = app.runtime.store.read(|session| {
        let total =
            store::repository_commits_count(session, repository_id, reachable_only, linked_only)?;
        let commits = store::repository_commits_page(
            session,
            repository_id,
            reachable_only,
            linked_only,
            limit,
            offset,
        )?;
        let links = store::links_with_task_keys(session, &ids_of(&commits))?;
        Ok((total, commits, links))
    })?;
    let items: Vec<LogItem<'_>> = commits
        .iter()
        .map(|commit| LogItem {
            commit,
            tasks: links
                .iter()
                .filter(|row| row.link.commit_id == commit.id)
                .map(|row| TaskRef {
                    task_id: row.link.task_id,
                    task_key: row.task_key.clone(),
                    source: row.link.source.clone(),
                    confidence: row.link.confidence.0,
                })
                .collect(),
        })
        .collect();
    Ok(reads::repository_commits_value(
        repository_id,
        total,
        &items,
    ))
}

/// `GitService.commit_detail` (`git show`).
pub fn git_show(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    repository: &str,
    sha: &str,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::GitRead)?;
    let workspace_id = app.workspace_or_default(workspace)?;
    let repo = repository_row(app, workspace_id, repository, true)?;
    let repository_id = repo.repository.id;
    let prefix = sha.to_owned();
    let loaded = app.runtime.store.read(|session| {
        let Some(commit) = store::commits_by_sha_prefix(session, repository_id, &prefix)?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let files = store::file_changes(session, &[commit.id])?;
        let file_ids: Vec<Id> = files.iter().map(|file| file.id).collect();
        let symbols = store::commit_symbols(session, &file_ids)?;
        let analysis = store::commit_analyses(session, &[commit.id])?
            .into_iter()
            .next();
        let equivalences = store::equivalences_of_commit(session, commit.id)?;
        let links: Vec<(CommitTaskLink, String)> =
            store::links_with_task_keys(session, &[commit.id])?
                .into_iter()
                .map(|row| (row.link, row.task_key))
                .collect();
        Ok(Some((
            commit,
            files,
            symbols,
            analysis,
            equivalences,
            links,
        )))
    })?;
    let Some((commit, files, symbols, analysis, equivalences, links)) = loaded else {
        return Err(WgError::not_found(format!(
            "commit {sha} has not been ingested for {}",
            repo.repository.name
        )));
    };
    Ok(reads::commit_detail_value(&DetailInput {
        commit: &commit,
        files: &files,
        symbols: &symbols,
        analysis: analysis.as_ref(),
        equivalences: &equivalences,
        links: &links,
    }))
}

/// `GitService.link_commit` (`git link`): the commit is ingested on demand from the task's
/// repositories (or the named one), then linked with confidence 1.0.
pub fn git_link(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_reference: &str,
    sha: &str,
    repository: Option<&str>,
    note: &str,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::GitLink)?;
    let workspace_id = app.workspace_or_default(workspace)?;
    let task = task_row(app, Some(workspace_id), task_reference)?;
    let candidates: Vec<Id> = match repository {
        Some(reference) => vec![
            repository_row(app, workspace_id, reference, false)?
                .repository
                .id,
        ],
        None => app.runtime.store.read(|session| {
            let world = session.task_world(Some(workspace_id), &[])?;
            let mut ids: Vec<Id> = world
                .tasks
                .task_repositories
                .values()
                .filter(|row| row.task_id == task.id)
                .map(|row| row.repository_id)
                .collect();
            if ids.is_empty() {
                ids = world
                    .tasks
                    .registry
                    .repositories
                    .values()
                    .filter(|repo| repo.workspace_id == workspace_id && repo.archived_at.is_none())
                    .map(|repo| repo.id)
                    .collect();
            }
            Ok(ids)
        })?,
    };
    let mut found: Option<(Id, Id)> = None;
    for repository_id in candidates {
        if let Some(commit_id) = ensure_commit(app, context, repository_id, sha)? {
            found = Some((repository_id, commit_id));
            break;
        }
    }
    let Some((repository_id, commit_id)) = found else {
        return Err(WgError::not_found(format!(
            "commit {sha} not found in the task's repositories"
        )));
    };
    let repository_name_access = app.runtime.store.read(|session| {
        let repo = session
            .fetch::<Repository>(&RowKey::Repository(repository_id))?
            .ok_or_else(|| WgError::not_found("repository not found"))?;
        let exists = session
            .fetch::<wg_model::entity::task::TaskRepository>(&RowKey::TaskRepository {
                task_id: task.id,
                repository_id,
            })?
            .is_some();
        let links = store::links_with_task_keys(session, &[commit_id])?;
        let commit = store::commits_by_ids(session, &[commit_id])?
            .into_iter()
            .next()
            .ok_or_else(|| WgError::not_found("commit not found"))?;
        let existing: Vec<CommitTaskLink> = links.into_iter().map(|row| row.link).collect();
        Ok((repo, exists, commit, existing))
    })?;
    let (repo, task_repository_exists, commit, existing) = repository_name_access;
    let access = repo.access_mode.as_str().to_owned();
    let actor = context.actor.to_string();
    let attempt_id = context.attempt;
    let outcome = app.runtime.decide(
        context,
        |_session| Ok(()),
        |_: &(), stamp| {
            let link_id = stamp.id(0);
            let event_id = stamp.id(1);
            let writes = links::explicit_link_writes(
                &commit,
                task.id,
                repository_id,
                &access,
                &existing,
                ExplicitLinkIds {
                    id: link_id,
                    note,
                    actor: &actor,
                    attempt_id,
                    task_repository_exists,
                    now: stamp.at,
                },
            );
            let mut decision = Decision::new(());
            decision.writes = writes.into_iter().collect();
            decision.events = std::iter::once(EventDraft::new(
                stamp,
                event_id,
                Some(task.workspace_id),
                task.id,
                EventPayload::TaskCommitLinked(TaskCommitLinked {
                    commit_id: commit.id,
                    sha: commit.sha.clone(),
                    repository_id,
                    source: links::EXPLICIT.to_owned(),
                }),
            ))
            .collect();
            Ok(decision)
        },
    )?;
    // The reply is read back from the store, as the Python service reads the links it returns.
    let _ = outcome;
    let stored: Vec<CommitTaskLink> = app.runtime.store.read(|session| {
        Ok(store::links_with_task_keys(session, &[commit.id])?
            .into_iter()
            .map(|row| row.link)
            .collect())
    })?;
    Ok(links::link_outputs(&stored, &commit, task.id))
}

/// `GitService.unlink_commit` (`git unlink`): removes the explicit links of the task whose
/// commit SHA starts with `sha`.
pub fn git_unlink(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_reference: &str,
    sha: &str,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::GitLink)?;
    let workspace_id = app.workspace_or_default(workspace)?;
    let task = task_row(app, Some(workspace_id), task_reference)?;
    let task_id = task.id;
    let sha_owned = sha.to_owned();
    let removed = app.runtime.decide(
        context,
        |session| {
            let rows = store::task_commit_links(session, task_id, 0.0)?;
            Ok(rows
                .into_iter()
                .map(|row| (row.link, row.commit.sha))
                .collect::<Vec<(CommitTaskLink, String)>>())
        },
        |links: &Vec<(CommitTaskLink, String)>, stamp| {
            let writes = links::unlink_selection(links, task_id, &sha_owned);
            let removed = writes.len();
            let mut decision = Decision::new(removed);
            decision.writes = writes.into_iter().collect();
            if removed > 0 {
                let event = EventDraft::new(
                    stamp,
                    stamp.id(0),
                    Some(task.workspace_id),
                    task_id,
                    links::commit_unlinked_payload(&sha_owned, removed),
                );
                decision.events = std::iter::once(event).collect();
            }
            Ok(decision)
        },
    )?;
    Ok(dict!({
        "removed": wg_wire::convention::int(i64::try_from(removed.reply).unwrap_or(i64::MAX)),
    }))
}

/// `GitService.task_changes` (`git changes`): the linked commits, the cumulative churn and the
/// change surfaces of the task.
pub fn git_changes(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_reference: &str,
    min_confidence: f64,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::GitRead)?;
    let scope = app.workspace_or_all(workspace)?;
    let reference = task_reference.to_owned();
    let (task, links, files, analyses, surfaces) = app.runtime.store.read(|session| {
        let world = session.task_world(scope, &[])?;
        let task = resolve::task(&world.tasks, scope, &reference)?.clone();
        let links = store::task_commit_links(session, task.id, min_confidence)?;
        let commit_ids = unique_commit_ids(links.iter().map(|row| row.commit.id));
        let files = store::file_changes(session, &commit_ids)?;
        let analyses = store::commit_analyses(session, &commit_ids)?;
        let surfaces = store::task_change_surfaces(session, task.id)?;
        Ok((task, links, files, analyses, surfaces))
    })?;
    let grouped = group_by_commit(&links);
    let link_refs: Vec<Vec<&CommitTaskLink>> = grouped
        .iter()
        .map(|entry| entry.links.iter().collect())
        .collect();
    let commits: Vec<ChangeCommit<'_>> = grouped
        .iter()
        .zip(&link_refs)
        .map(|(entry, refs)| ChangeCommit {
            commit: &entry.commit,
            repository_name: &entry.repository_name,
            links: refs,
        })
        .collect();
    let surface_refs: Vec<&TaskChangeSurface> = surfaces.iter().collect();
    let task_key = task.key.to_string();
    Ok(reads::task_changes_value(&ChangesInput {
        task_id: task.id,
        task_key: &task_key,
        min_confidence,
        commits: &commits,
        files: &files,
        analyses: &analyses,
        surfaces: &surface_refs,
    }))
}

/// `GitService.task_commits` (`GET /tasks/{ref}/commits`): the task's linked commits at or above
/// `min_confidence`, each with the links that attribute it, in commit-date order.
pub fn task_commits(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_reference: &str,
    min_confidence: f64,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::GitRead)?;
    let scope = app.workspace_or_all(workspace)?;
    let reference = task_reference.to_owned();
    let grouped = app.runtime.store.read(|session| {
        let world = session.task_world(scope, &[])?;
        let task = resolve::task(&world.tasks, scope, &reference)?.clone();
        let links = store::task_commit_links(session, task.id, min_confidence)?;
        Ok(group_by_commit(&links))
    })?;
    Ok(reads::task_commits_value(&grouped))
}

/// `GitService.change_sets` (`GET /tasks/{ref}/change-sets`): the task's change sets, oldest
/// first, each with its repository items.
pub fn change_sets(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_reference: &str,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::GitRead)?;
    let scope = app.workspace_or_all(workspace)?;
    let reference = task_reference.to_owned();
    let sets = app.runtime.store.read(|session| {
        let world = session.task_world(scope, &[])?;
        let task = resolve::task(&world.tasks, scope, &reference)?.clone();
        session
            .change_sets_of_task(task.id)?
            .into_iter()
            .map(|set| {
                let items = session.change_set_items(set.id)?;
                Ok((set, items))
            })
            .collect::<Result<Vec<_>, WgError>>()
    })?;
    Ok(reads::change_sets_value(&sets))
}

/// `GitService.task_diff` (`git diff`): the final diff of each change surface, or the diff of
/// each linked commit when the task has no surface. Text is read live from git and cut to
/// `max_bytes` characters across the parts.
pub fn git_diff(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_reference: &str,
    repository: Option<&str>,
    max_bytes: usize,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::GitRead)?;
    let scope = app.workspace_or_all(workspace)?;
    let reference = task_reference.to_owned();
    let repository_reference = repository.map(str::to_owned);
    let loaded = app.runtime.store.read(|session| {
        let world = session.task_world(scope, &[])?;
        let task = resolve::task(&world.tasks, scope, &reference)?.clone();
        let registry = session.registry_world(Some(task.workspace_id))?;
        let repo_filter = match &repository_reference {
            Some(name) => Some(resolve::repository(&registry, task.workspace_id, name, true)?.id),
            None => None,
        };
        let surfaces = store::task_change_surfaces(session, task.id)?;
        let links = store::task_commit_links(session, task.id, 0.5)?;
        let mut repos = Vec::new();
        for repo in registry
            .repositories
            .values()
            .filter(|repo| repo.workspace_id == task.workspace_id)
        {
            let checkouts = session.checkouts(repo.id)?;
            let path = wg_registry::checkout::primary_checkout(&checkouts, true)
                .map(|checkout| checkout.path.clone());
            repos.push(RepoEntry {
                id: repo.id,
                name: repo.name.clone(),
                path,
            });
        }
        Ok((task, repo_filter, surfaces, links, repos))
    })?;
    let (task, repo_filter, surfaces, links, repos) = loaded;
    let task_key = task.key.to_string();
    let names: BTreeMap<Id, String> = repos.iter().map(|r| (r.id, r.name.clone())).collect();
    let paths: BTreeMap<Id, String> = repos
        .iter()
        .filter_map(|r| r.path.clone().map(|path| (r.id, path)))
        .collect();
    let mut parts: Vec<DiffPart> = Vec::new();
    let mut missing: Vec<Missing> = Vec::new();
    if !surfaces.is_empty() {
        for surface in &surfaces {
            if repo_filter.is_some_and(|wanted| wanted != surface.repository_id) {
                continue;
            }
            let Some(path) = paths.get(&surface.repository_id) else {
                continue;
            };
            let repository_name = names
                .get(&surface.repository_id)
                .cloned()
                .unwrap_or_default();
            let git = Git {
                path: PathBuf::from(path),
                timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
            };
            let base = surface.base_sha.as_str();
            let final_sha = surface.final_sha.as_str();
            if !(git.object_exists(base).unwrap_or(false)
                && git.object_exists(final_sha).unwrap_or(false))
            {
                missing.push(Missing::Range {
                    repository: repository_name,
                    range: format!("{base}..{final_sha}"),
                });
                continue;
            }
            let text = git
                .run_with(&["diff", "-M", base, final_sha], RunOptions::CHECKED)
                .map_err(git_err)?
                .stdout;
            parts.push(DiffPart::Final {
                repository: repository_name,
                range: format!("{}..{}", short_sha(base), short_sha(final_sha)),
                diff: text,
            });
        }
    } else {
        for entry in group_by_commit(&links) {
            if repo_filter.is_some_and(|wanted| wanted != entry.commit.repository_id) {
                continue;
            }
            let Some(path) = paths.get(&entry.commit.repository_id) else {
                continue;
            };
            let git = Git {
                path: PathBuf::from(path),
                timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
            };
            let sha = entry.commit.sha.clone();
            if !git.object_exists(&sha).unwrap_or(false) {
                missing.push(Missing::Commit {
                    repository: entry.repository_name.clone(),
                    sha,
                });
                continue;
            }
            let text = git
                .run_with(
                    &[
                        "show",
                        "--format=commit %H%n%s%n",
                        "-M",
                        "--first-parent",
                        sha.as_str(),
                    ],
                    RunOptions::CHECKED,
                )
                .map_err(git_err)?
                .stdout;
            parts.push(DiffPart::Commit {
                repository: entry.repository_name.clone(),
                sha,
                diff: text,
            });
        }
    }
    if !missing.is_empty() {
        let gap_context = format!("task diff {task_key}");
        let gaps = missing_gap_targets(
            &missing,
            &repos
                .iter()
                .map(|r| (r.id, r.name.clone()))
                .collect::<Vec<_>>(),
        );
        record_gaps(app, context, &gaps, &gap_context)?;
    }
    Ok(reads::task_diff_value(
        task.id, &task_key, &parts, &missing, max_bytes,
    ))
}

/// `GitService.record_gaps`: remembers SHAs whose objects are missing, once per repository.
fn record_gaps(
    app: &mut App,
    context: &RequestContext,
    gaps: &[(Id, String)],
    gap_context: &str,
) -> Result<(), WgError> {
    if gaps.is_empty() {
        return Ok(());
    }
    let pending: Vec<(Id, String)> = app.runtime.store.read(|session| {
        let mut found = Vec::new();
        for (repository_id, sha) in gaps {
            if store::git_object_gap(session, *repository_id, sha)?.is_none() {
                found.push((*repository_id, sha.clone()));
            }
        }
        Ok(found)
    })?;
    if pending.is_empty() {
        return Ok(());
    }
    let gap_context = gap_context.to_owned();
    app.runtime
        .decide(
            context,
            |_session| Ok(()),
            |_: &(), stamp| {
                let mut seq = stamp.ids();
                let mut decision = Decision::new(());
                let mut seen: BTreeSet<(Id, String)> = BTreeSet::new();
                let mut writes = Vec::new();
                for (repository_id, sha) in &pending {
                    if !seen.insert((*repository_id, sha.clone())) {
                        continue;
                    }
                    let (id, next) = seq.next();
                    seq = next;
                    writes.push(Write::Insert(Row::GitObjectGap(GitObjectGap {
                        id,
                        repository_id: *repository_id,
                        sha: sha.clone(),
                        context: gap_context.clone(),
                        detected_at: stamp.at,
                    })));
                }
                decision.writes = writes.into_iter().collect();
                Ok(decision)
            },
        )
        .map(|_| ())
}

/// `GitService.compute_change_surface` (`git surface`): measures what the task changed in each
/// repository, from the attempt baseline to the accepted revision (or from the linked commits
/// when a manifest is missing), and stores the surfaces.
pub fn git_surface(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_reference: &str,
) -> Result<Value, WgError> {
    app.runtime
        .stamps
        .stamp(context)?
        .require(&[Permission::TaskRead])?;
    let scope = app.workspace_or_default(workspace)?;
    let task = task_row(app, Some(scope), task_reference)?;
    let task_id = task.id;
    let input = app
        .runtime
        .store
        .read(|session| surface_inputs(session, &task))?;
    let mut repo_ids: BTreeSet<Id> = input.final_entries.keys().copied().collect();
    let mut by_repo: BTreeMap<Id, Vec<String>> = BTreeMap::new();
    for row in &input.links {
        if !row.commit.is_merge {
            by_repo
                .entry(row.commit.repository_id)
                .or_default()
                .push(row.commit.sha.clone());
        }
    }
    repo_ids.extend(by_repo.keys().copied());
    let mut records: Vec<(Id, SurfaceRecord)> = Vec::new();
    let mut gaps: Vec<(Id, String)> = Vec::new();
    for repo_id in repo_ids {
        let Some(path) = input.paths.get(&repo_id) else {
            continue;
        };
        let git = Git {
            path: PathBuf::from(path),
            timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
        };
        let linked: Vec<String> = by_repo.get(&repo_id).cloned().unwrap_or_default();
        let plan = plan_surface(
            &SurfaceRepo {
                manifest_base: input.base_entries.get(&repo_id).map(String::as_str),
                manifest_final: input.final_entries.get(&repo_id).map(String::as_str),
                linked_shas: &linked,
            },
            |sha| git.object_exists(sha).unwrap_or(false),
            |ancestor, descendant| git.is_ancestor(ancestor, descendant).unwrap_or(false),
            |sha| {
                git.run_with(
                    &["rev-parse", "--verify", "--quiet", &format!("{sha}^")],
                    NO_CHECK_OPTIONS,
                )
                .ok()
                .and_then(|output| wg_gitcore::non_empty_trimmed(&output.stdout))
            },
        );
        match plan {
            SurfacePlan::Skip => {}
            SurfacePlan::Gap { final_rev, .. } => gaps.push((repo_id, final_rev)),
            SurfacePlan::Measure {
                base,
                final_rev,
                provenance,
            } => {
                // The shell reads the file list, then each text file's hunks and sources; the
                // surface is decided by `surface::surface_from_inputs`.
                let numstat = git
                    .run_with(
                        &["diff", "--numstat", "-z", "-M", &base, &final_rev],
                        RunOptions::CHECKED,
                    )
                    .map_err(git_err)?
                    .stdout;
                let files: Vec<FileChange> = parse_numstat_z(&numstat);
                let mut inputs: Vec<Option<FileInput>> = Vec::with_capacity(files.len());
                for file in &files {
                    if file.binary {
                        inputs.push(None);
                        continue;
                    }
                    let hunks = git
                        .run_with(
                            &surface::hunk_args(&base, &final_rev, file),
                            NO_CHECK_OPTIONS,
                        )
                        .map_err(git_err)?
                        .stdout;
                    let old_source = git.show_file(&base, surface::base_path(file));
                    let new_source = git.show_file(&final_rev, &file.path);
                    inputs.push(Some(FileInput {
                        hunks,
                        old_source,
                        new_source,
                    }));
                }
                let record = surface::surface_from_inputs(
                    &base,
                    &final_rev,
                    provenance,
                    &files,
                    &inputs,
                    extract_symbols,
                );
                records.push((repo_id, record));
            }
        }
    }
    if !gaps.is_empty() {
        let gap_context = format!("change surface {task_id}");
        record_gaps(app, context, &gaps, &gap_context)?;
    }
    let replies: Vec<Value> = records
        .iter()
        .map(|(repo_id, record)| analysis::surface_value(*repo_id, record))
        .collect();
    let existing = input.existing_surfaces;
    let attempt_id = input.attempt_id;
    let workspace_id = task.workspace_id;
    app.runtime.decide(
        context,
        |_session| Ok(()),
        |_: &(), stamp| {
            let mut seq = stamp.ids();
            let mut decision = Decision::new(());
            let mut writes = Vec::new();
            for (repo_id, record) in &records {
                let detail = analysis::surface_detail(record);
                match existing.iter().find(|row| row.repository_id == *repo_id) {
                    Some(row) => {
                        let mut updated = analysis::surface_row(
                            row.id, task_id, *repo_id, attempt_id, record, detail, stamp.at,
                        );
                        updated.computed_at = row.computed_at;
                        writes.push(Write::Update {
                            row: Row::TaskChangeSurface(updated),
                            expected_version: None,
                        });
                    }
                    None => {
                        let (id, next) = seq.next();
                        seq = next;
                        writes.push(Write::Insert(Row::TaskChangeSurface(
                            analysis::surface_row(
                                id, task_id, *repo_id, attempt_id, record, detail, stamp.at,
                            ),
                        )));
                    }
                }
            }
            decision.writes = writes.into_iter().collect();
            if !records.is_empty() {
                let (event_id, _) = seq.next();
                let insertions: i64 = records.iter().map(|(_, r)| to_i64(r.insertions)).sum();
                let deletions: i64 = records.iter().map(|(_, r)| to_i64(r.deletions)).sum();
                let event = EventDraft::new(
                    stamp,
                    event_id,
                    Some(workspace_id),
                    task_id,
                    EventPayload::TaskChangeSurfaceComputed(TaskChangeSurfaceComputed {
                        repositories: to_i64(records.len()),
                        insertions,
                        deletions,
                    }),
                );
                decision.events = std::iter::once(event).collect();
            }
            Ok(decision)
        },
    )?;
    Ok(Value::List(replies))
}

/// `GitService.analyze_commits` through `GitService.repository_commits` (`git analyze`).
pub fn git_analyze(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    repository: &str,
    limit: i64,
) -> Result<Value, WgError> {
    let workspace_id = app.workspace_or_default(workspace)?;
    let repo = repository_row(app, workspace_id, repository, true)?;
    let repository_id = repo.repository.id;
    let commits = app.runtime.store.read(|session| {
        store::repository_commits_page(session, repository_id, true, false, limit, 0)
    })?;
    let mut analyzed: i64 = 0;
    for commit in commits.iter().filter(|commit| !commit.is_merge) {
        if analyze_commit(app, context, commit.id)? {
            analyzed += 1;
        }
    }
    Ok(dict!({ "analyzed": int(analyzed) }))
}

/// `GitService.analyze_commit`: the structure analysis of one commit's files. Returns whether
/// an analysis was written.
pub fn analyze_commit(
    app: &mut App,
    context: &RequestContext,
    commit_id: Id,
) -> Result<bool, WgError> {
    let loaded = app.runtime.store.read(|session| {
        let Some(commit) = store::commits_by_ids(session, &[commit_id])?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        if commit.is_merge || !store::commit_analyses(session, &[commit_id])?.is_empty() {
            return Ok(None);
        }
        let Some(repository) =
            session.fetch::<Repository>(&RowKey::Repository(commit.repository_id))?
        else {
            return Ok(None);
        };
        let checkouts = session.checkouts(repository.id)?;
        let Some(path) = wg_registry::checkout::primary_checkout(&checkouts, true)
            .map(|checkout| checkout.path.clone())
        else {
            return Ok(None);
        };
        let files = store::file_changes(session, &[commit_id])?;
        Ok(Some((commit, path, files)))
    })?;
    let Some((commit, path, files)) = loaded else {
        return Ok(false);
    };
    let git = Git {
        path: PathBuf::from(&path),
        timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
    };
    if !git.object_exists(&commit.sha).unwrap_or(false) {
        let unavailable = Commit {
            object_available: false,
            ..commit.clone()
        };
        apply_commit_updates_now(app, context, unavailable)?;
        return Ok(false);
    }
    let parent = commit.parents.first().cloned();
    let mut results: Vec<FileResult> = Vec::new();
    for file in &files {
        if file.is_binary {
            results.push(FileResult {
                file_change_id: file.id,
                structure: None,
            });
            continue;
        }
        let base = parent.clone().unwrap_or_else(|| EMPTY_TREE.to_owned());
        let old_path = file.old_path.clone();
        let mut args: Vec<&str> = vec![
            "diff",
            "-U0",
            "--no-color",
            "-M",
            base.as_str(),
            commit.sha.as_str(),
            "--",
        ];
        if let Some(old) = old_path.as_deref() {
            args.push(old);
        }
        args.push(file.path.as_str());
        let hunks = git
            .run_with(&args, NO_CHECK_OPTIONS)
            .map_err(git_err)?
            .stdout;
        let (deleted, added) = parse_hunks(&hunks);
        let old_source = match &parent {
            Some(parent_sha) if file.change_type != "A" => {
                git.show_file(parent_sha, old_path.as_deref().unwrap_or(&file.path))
            }
            _ => None,
        };
        let new_source = if file.change_type != "D" {
            git.show_file(&commit.sha, &file.path)
        } else {
            None
        };
        let structure = analyze_file(
            &file.path,
            file.language.as_deref(),
            old_source.as_deref(),
            new_source.as_deref(),
            &deleted,
            &added,
            extract_symbols,
        );
        results.push(FileResult {
            file_change_id: file.id,
            structure: Some(structure),
        });
    }
    let outcome = app.runtime.decide(
        context,
        |session| Ok(!store::commit_analyses(session, &[commit_id])?.is_empty()),
        |exists_already: &bool, stamp| {
            if *exists_already {
                return Ok(Decision::new(false));
            }
            let (_, writes) = analysis::analysis_writes(&commit, &results, stamp.ids(), stamp.at);
            let mut decision = Decision::new(true);
            decision.writes = writes.into_iter().collect();
            Ok(decision)
        },
    )?;
    Ok(outcome.reply)
}

/// Writes one commit row in its own transaction.
fn apply_commit_updates_now(
    app: &mut App,
    context: &RequestContext,
    commit: Commit,
) -> Result<(), WgError> {
    app.runtime
        .decide(
            context,
            |_session| Ok(()),
            |_: &(), _stamp| {
                let mut decision = Decision::new(());
                decision.writes = std::iter::once(Write::Update {
                    row: Row::Commit(commit.clone()),
                    expected_version: None,
                })
                .collect();
                Ok(decision)
            },
        )
        .map(|_| ())
}

/// `ensure_commit`: the id of the commit `sha` in the repository, ingesting it (and analysing
/// it) when git has it but the store does not. `None` when the repository or git cannot give it.
pub(crate) fn ensure_commit(
    app: &mut App,
    context: &RequestContext,
    repository_id: Id,
    sha: &str,
) -> Result<Option<Id>, WgError> {
    let Ok(state) = repo_state(app, repository_id) else {
        return Ok(None);
    };
    let git = Git {
        path: PathBuf::from(&state.path),
        timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
    };
    let Ok(full) = git.resolve_commit(sha) else {
        return Ok(None);
    };
    let existing = app.runtime.store.read(|session| {
        Ok(store::commit_by_sha(session, repository_id, &full)?.map(|commit| commit.id))
    })?;
    if existing.is_some() {
        return Ok(existing);
    }
    let created = ingest_batch(app, context, &git, &state, &[full])?;
    let Some((first, _)) = created.first().cloned() else {
        return Ok(None);
    };
    for (commit_id, _) in &created {
        analyze_commit(app, context, *commit_id)?;
    }
    Ok(Some(first))
}

/// `WorktreeManager.list` (`git worktrees`), newest first.
pub fn git_worktrees(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    status: Option<&str>,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::RepoRead)?;
    let scope = app.workspace_or_all(workspace)?;
    let wanted = status.map(str::to_owned);
    let mut rows = app
        .runtime
        .store
        .read(|session| session.worktrees_with_repository(scope, None))?;
    rows.retain(|row| {
        wanted
            .as_deref()
            .is_none_or(|status| row.worktree.status.as_str() == status)
    });
    rows.sort_by_key(|row| std::cmp::Reverse(row.worktree.created_at));
    let views: Vec<WorktreeRow<'_>> = rows
        .iter()
        .map(|row| WorktreeRow {
            worktree: &row.worktree,
            repository: &row.repository_name,
        })
        .collect();
    Ok(worktrees::list_value(&views))
}

/// `WorktreeManager.cleanup` (`git cleanup-worktrees`): a dry run lists the released
/// worktrees; `apply` removes the clean ones.
pub fn git_cleanup_worktrees(
    app: &mut App,
    context: &RequestContext,
    apply: bool,
) -> Result<Value, WgError> {
    git_require(app, context, Permission::RepoAdmin)?;
    let mut rows = app
        .runtime
        .store
        .read(|session| session.worktrees_with_repository(None, Some(WorktreeStatus::Released)))?;
    rows.retain(|row| worktrees::is_cleanup_candidate(&row.worktree));
    rows.sort_by_key(|row| std::cmp::Reverse(row.worktree.created_at));
    if !apply {
        let views: Vec<WorktreeRow<'_>> = rows
            .iter()
            .map(|row| WorktreeRow {
                worktree: &row.worktree,
                repository: &row.repository_name,
            })
            .collect();
        return Ok(worktrees::cleanup_dry_run_value(&views));
    }
    let root = app.paths.worktrees_dir().to_string();
    let roots = configured_roots(app)?;
    let mut results = Vec::new();
    for row in rows {
        results.push(remove_released_worktree(
            app,
            context,
            row.worktree,
            &root,
            &roots,
        )?);
    }
    Ok(Value::List(results))
}

/// `WorktreeManager.remove` for one released worktree (never forced).
fn remove_released_worktree(
    app: &mut App,
    context: &RequestContext,
    worktree: ManagedWorktree,
    root: &str,
    roots: &[PathBuf],
) -> Result<Value, WgError> {
    let path = PathBuf::from(&worktree.path);
    let source = app.runtime.store.read(|session| {
        Ok(session
            .fetch::<wg_model::entity::repository::RepositoryCheckout>(
                &RowKey::RepositoryCheckout(worktree.source_checkout_id),
            )?
            .map(|checkout| checkout.path))
    })?;
    let id = worktree.id.to_string();
    if !is_within(&path.to_string_lossy(), root) && !in_attempt_dir(&path, roots) {
        return Err(WgError::validation(format!(
            "refusing to remove {}: not inside Workgraph's worktree directory",
            path.display()
        )));
    }
    if path.exists() {
        let git = Git {
            path: PathBuf::from(path.display().to_string()),
            timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
        };
        if git.is_repository().unwrap_or(false) && !git.is_clean().unwrap_or(false) {
            return Ok(removal_result(
                &id,
                false,
                Some("worktree has uncommitted changes"),
            ));
        }
        if let Some(source_path) = source.as_deref() {
            let source_git = Git {
                path: PathBuf::from(source_path),
                timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
            };
            if source_git.is_repository().unwrap_or(false) {
                let output = source_git
                    .worktree_remove(&worktree.path, false, NO_CHECK_OPTIONS)
                    .map_err(git_err)?;
                if output.returncode != 0 && path.exists() {
                    let reason: String = prefix(output.stderr.trim(), 300);
                    return Ok(removal_result(&id, false, Some(&reason)));
                }
            }
        }
    }
    if let Some(source_path) = source.as_deref() {
        let source_git = Git {
            path: PathBuf::from(source_path),
            timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
        };
        if source_git.is_repository().unwrap_or(false) {
            let _ = source_git.worktree_prune(NO_CHECK_OPTIONS);
        }
    }
    let worktree_path = worktree.path.clone();
    let branch = worktree.branch.clone();
    let workspace_id = worktree.workspace_id;
    let attempt_id = worktree.attempt_id;
    let worktree_id = worktree.id;
    app.runtime.decide(
        context,
        |_session| Ok(()),
        |_: &(), stamp| {
            let mut decision = Decision::new(());
            let removed = ManagedWorktree {
                status: WorktreeStatus::Removed,
                removed_at: Some(stamp.at),
                ..worktree.clone()
            };
            decision.writes = std::iter::once(Write::Update {
                row: Row::ManagedWorktree(removed),
                expected_version: None,
            })
            .collect();
            let event = EventDraft::new(
                stamp,
                stamp.id(0),
                Some(workspace_id),
                worktree_id,
                EventPayload::WorktreeRemoved(wg_model::event::WorktreeRemoved {
                    path: worktree_path.clone(),
                    branch_kept: branch.to_string(),
                }),
            )
            .with_attempt(attempt_id);
            decision.events = std::iter::once(event).collect();
            Ok(decision)
        },
    )?;
    remove_empty_attempt_dir(&path, Path::new(root), roots);
    Ok(removal_result(&id, true, None))
}

/// `git install-hook`: installs the optional post-commit hook in the first available checkout.
pub fn git_install_hook(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    repository: &str,
) -> Result<Value, CliFailure> {
    let _ = context;
    let workspace_id = app.workspace_or_default(workspace)?;
    let repo = repository_row(app, workspace_id, repository, true)?;
    let repository_id = repo.repository.id;
    let checkout_path = app.runtime.store.read(|session| {
        Ok(session
            .checkouts(repository_id)?
            .into_iter()
            .find(|checkout| checkout.available && checkout.kind != "managed_worktree")
            .map(|checkout| checkout.path))
    })?;
    // Python: `fail("no available checkout", 4)`, a `cli_error` rather than `not_found`.
    let Some(path) = checkout_path else {
        return Err(CliFailure::fail("no available checkout", 4));
    };
    let git = Git {
        path: PathBuf::from(&path),
        timeout_ms: wg_runtime::spec::effects::GIT_TIMEOUT_MS,
    };
    match wg_git::install_post_commit_hook(&git, &repository_id.to_string()) {
        Ok(wg_git::HookOutcome::Installed(hook)) => {
            Ok(dict!({ "hook": text(&hook.display().to_string()) }))
        }
        // Python: `fail(f"{hook} already exists ...", 3)`, a `cli_error` with no details.
        Ok(wg_git::HookOutcome::Refused(hook)) => Err(CliFailure::fail(
            format!(
                "{} already exists and is not managed by Workgraph; not overwriting",
                hook.display()
            ),
            3,
        )),
        Err(err) => Err(WgError::git(err.to_string()).into()),
    }
}

/// A repository of the workspace with its primary checkout path, for the diff.
struct RepoEntry {
    id: Id,
    name: String,
    path: Option<String>,
}

/// The inputs of the change surface computation, loaded in one read.
struct SurfaceInputs {
    attempt_id: Option<Id>,
    final_entries: BTreeMap<Id, String>,
    base_entries: BTreeMap<Id, String>,
    links: Vec<TaskCommitLinkRow>,
    paths: BTreeMap<Id, String>,
    existing_surfaces: Vec<TaskChangeSurface>,
}

/// Loads the accepted and baseline manifests, the linked commits, the checkouts and the surfaces
/// a task's surface computation reads (`compute_change_surface`'s first half).
fn surface_inputs(
    session: &mut Session<'_>,
    task: &wg_model::entity::task::Task,
) -> Result<SurfaceInputs, WgError> {
    let acceptances = session.task_acceptances(task.id)?;
    let accepted = acceptances
        .iter()
        .filter(|row| row.decision == "accepted")
        .max_by_key(|row| row.created_at)
        .cloned();
    let final_entries = match accepted.as_ref().and_then(|row| row.manifest_id) {
        Some(manifest_id) => manifest_shas(session.manifest_by_id(manifest_id)?),
        None => BTreeMap::new(),
    };
    let attempts = session.attempts_of_task(task.id, &[])?;
    let first = attempts
        .iter()
        .filter(|attempt| attempt.baseline_manifest_id.is_some())
        .min_by_key(|attempt| attempt.number)
        .and_then(|attempt| attempt.baseline_manifest_id);
    let base_entries = match first {
        Some(manifest_id) => manifest_shas(session.manifest_by_id(manifest_id)?),
        None => BTreeMap::new(),
    };
    let links = store::task_commit_links(session, task.id, 0.5)?;
    let registry = session.registry_world(Some(task.workspace_id))?;
    let mut paths = BTreeMap::new();
    for repo in registry
        .repositories
        .values()
        .filter(|repo| repo.workspace_id == task.workspace_id)
    {
        let checkouts = session.checkouts(repo.id)?;
        if let Some(checkout) = wg_registry::checkout::primary_checkout(&checkouts, true) {
            paths.insert(repo.id, checkout.path.clone());
        }
    }
    let existing_surfaces = store::task_change_surfaces(session, task.id)?;
    Ok(SurfaceInputs {
        attempt_id: accepted.and_then(|row| row.attempt_id),
        final_entries,
        base_entries,
        links,
        paths,
        existing_surfaces,
    })
}
