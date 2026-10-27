//! `git sync` (IF-12): the ingestion pipeline of one repository, as the shell runs it. Git is
//! read outside any transaction; each step then decides in one `BEGIN IMMEDIATE` transaction with
//! the pure functions of `wg_runtimespec::git`. Ported from `GitService.sync` and its helpers in
//! `integrations/git/service.py`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::spec::git_error;
use crate::spec::ingest::{self, IngestedCommit, SyncResult};
use crate::spec::ingest::{
    count, diff_line, equivalence_facts as to_facts, offset_time, sync_result_value as result_value,
};
use crate::spec::links::{self as attribution, LinkDraft};
use crate::spec::mode::sync_is_full;
use wg_git::{Git, RunOptions};
use wg_gitcore::{
    CarriedLink, CommitDiff, CommitFacts, CommitMeta, Equivalence, OriginalLink,
    cherry_picked_from, parse_diff_tree, parse_meta, parse_patch_ids,
};
use wg_json::Value;
use wg_model::entity::git::{Commit, CommitEquivalence, CommitTaskLink};
use wg_model::entity::repository::{Repository, RepositoryRef};
use wg_model::event::{EventPayload, RepositoryChangesDetected};
use wg_model::{Decision, EventDraft, Row, RowKey, Write};
use wg_registry::resolve;
use wg_runtimespec::RequestContext;
use wg_types::{ErrorCode, Id, Permission, WgError, parse_id};

use wg_runtime::App;

use super::entries::repository_row;
use super::store;
use crate::spec::ingest::equivalence_evidence;
use crate::spec::links::carried_link_row;

/// The batch size of ingestion (`BATCH`).
const BATCH: usize = 400;

/// A commit to attribute, with its candidate task references (and the task each resolved to) and
/// the links it already has.
type CommitPlan = (
    Commit,
    Vec<(wg_gitcore::Candidate, Option<Id>)>,
    Vec<CommitTaskLink>,
);

/// The managed-worktree commits of a repository: SHA to (attempt, task, branch).
type Worktrees = BTreeMap<String, (Option<Id>, Id, String)>;
/// A git call that must not fail the caller on a non-zero exit.
const NO_CHECK: RunOptions<'static> = RunOptions {
    check: false,
    stdin: None,
    timeout_ms: None,
    env: &[],
};
/// The timeout of the sync's git calls, in milliseconds (`Git(path, timeout=600)`).
const SYNC_TIMEOUT_MS: u64 = 600_000;
/// The limit of listed ref names in `repository.changes_detected`.
const REF_LIST_LIMIT: usize = 100;
/// The number of local and tracked branches membership considers (`[:20]`).
const BRANCH_LIMIT: usize = 20;

/// What a sync needs to know about its repository before it runs git.
#[derive(Debug, Clone)]
pub(crate) struct RepoState {
    pub repository: Repository,
    pub workspace_id: Id,
    pub prefixes: Vec<String>,
    pub path: String,
    pub known_refs: BTreeMap<String, String>,
}

/// `GitService.sync_ref` and `sync_workspace` (`git sync`): one repository by reference, or
/// every unarchived repository of the workspace. A failing repository of a workspace sync is
/// reported as an `error: ...` result rather than failing the others.
pub fn git_sync(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    repository: Option<&str>,
    full: bool,
) -> Result<Value, WgError> {
    let workspace_id = app.workspace_or_default(workspace)?;
    let mode = if full { "full" } else { "incremental" };
    let results: Vec<Value> = match repository {
        Some(reference) => {
            let repo = repository_row(app, workspace_id, reference, false)?;
            let result = sync_repository(app, context, repo.repository.id, full)?;
            vec![result_value(&result)]
        }
        None => {
            let ids: Vec<Id> = app.runtime.store.read(|session| {
                Ok(session
                    .repositories(workspace_id, None, false)?
                    .into_iter()
                    .map(|repo| repo.id)
                    .collect())
            })?;
            let mut values = Vec::with_capacity(ids.len());
            for repository_id in ids {
                let value = match sync_repository(app, context, repository_id, full) {
                    Ok(result) => result_value(&result),
                    Err(err) => result_value(&SyncResult {
                        repository_id: repository_id.to_string(),
                        repository: format!("error: {}", err.message),
                        mode: mode.to_owned(),
                        ..SyncResult::default()
                    }),
                };
                values.push(value);
            }
            values
        }
    };
    Ok(Value::List(results))
}

/// `GitService._repo_info`: the repository, its primary checkout, its refs and every key
/// prefix its workspace's tasks use (the workspace prefix and the prefixes of existing keys).
pub(crate) fn repo_state(app: &mut App, repository_id: Id) -> Result<RepoState, WgError> {
    app.runtime.store.read(|session| {
        let repository = session
            .fetch::<Repository>(&RowKey::Repository(repository_id))?
            .ok_or_else(|| {
                WgError::new(
                    ErrorCode::NotFound,
                    format!("repository {repository_id} not found"),
                )
            })?;
        let checkouts = session.checkouts(repository_id)?;
        let path = wg_registry::checkout::primary_checkout(&checkouts, true)
            .map(|checkout| checkout.path.clone())
            .ok_or_else(|| {
                WgError::new(
                    ErrorCode::GitError,
                    format!("repository {} has no available checkout", repository.name),
                )
            })?;
        let known_refs = session
            .refs(repository_id)?
            .into_iter()
            .map(|row| (row.name, row.sha))
            .collect();
        let workspace = session
            .workspace_by_id(repository.workspace_id)?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "workspace not found"))?;
        let world = session.task_world(Some(repository.workspace_id), &[])?;
        let mut prefixes: BTreeSet<String> = BTreeSet::new();
        prefixes.insert(workspace.key_prefix.to_uppercase());
        for task in world.tasks.tasks.values() {
            let key = task.key.to_string();
            if let Some((prefix, _)) = key.split_once('-').filter(|(prefix, _)| !prefix.is_empty())
            {
                prefixes.insert(prefix.to_uppercase());
            }
        }
        Ok(RepoState {
            workspace_id: repository.workspace_id,
            prefixes: prefixes.into_iter().collect(),
            path,
            known_refs,
            repository,
        })
    })
}

/// `GitService.sync_ref` (`git sync` of one repository, the `mode` of the route): the repository
/// is resolved first (not found), then the caller's `git:ingest` permission is checked, then the
/// mode is validated, as the Python service orders them. Returns the sync result of the repository.
pub fn git_sync_ref(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    mode: &str,
) -> Result<Value, WgError> {
    let workspace_id = app.workspace_or_default(workspace)?;
    let repo = super::entries::repository_row(app, workspace_id, reference, false)?;
    app.runtime
        .stamps
        .stamp(context)?
        .require(&[Permission::GitIngest])?;
    let full = sync_is_full(mode)?;
    let result = sync_repository(app, context, repo.repository.id, full)?;
    Ok(result_value(&result))
}

/// `GitService.sync`: imports the commits of one repository and reconciles its refs.
pub(crate) fn sync_repository(
    app: &mut App,
    context: &RequestContext,
    repository_id: Id,
    full: bool,
) -> Result<SyncResult, WgError> {
    app.runtime
        .stamps
        .stamp(context)?
        .require(&[Permission::GitIngest])?;
    let state = repo_state(app, repository_id)?;
    let git = Git {
        path: PathBuf::from(&state.path),
        timeout_ms: SYNC_TIMEOUT_MS,
    };
    let refs: BTreeMap<String, String> = git.refs().map_err(git_error)?.into_iter().collect();
    let shallow = git.is_shallow().map_err(git_error)?;
    let mut result = SyncResult {
        repository_id: repository_id.to_string(),
        repository: state.repository.name.clone(),
        mode: if full { "full" } else { "incremental" }.to_owned(),
        shallow,
        head_refs: count(refs.len()),
        ..SyncResult::default()
    };
    let changed: Vec<String> = refs
        .iter()
        .filter(|(name, sha)| state.known_refs.get(*name) != Some(*sha))
        .map(|(name, _)| name.clone())
        .collect();
    let removed: Vec<String> = state
        .known_refs
        .keys()
        .filter(|name| !refs.contains_key(*name))
        .cloned()
        .collect();
    result.refs_changed = count(changed.len() + removed.len());
    let mut non_fast_forward = !removed.is_empty();
    for name in &changed {
        if let (Some(old), Some(new)) = (state.known_refs.get(name), refs.get(name))
            && !git.is_ancestor(old, new).map_err(git_error)?
        {
            non_fast_forward = true;
        }
    }

    let known: BTreeSet<String> = app.runtime.store.read(|session| {
        Ok(store::commit_shas(session, repository_id)?
            .into_iter()
            .collect())
    })?;
    let candidates = if refs.is_empty() {
        Vec::new()
    } else if full || state.known_refs.is_empty() {
        git.rev_list_all().map_err(git_error)?
    } else {
        let tips: Vec<String> = state
            .known_refs
            .values()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|sha| git.object_exists(sha).unwrap_or(false))
            .cloned()
            .collect();
        git.rev_list_all_excluding(&tips).map_err(git_error)?
    };
    let new_shas = ingest::new_shas(&candidates, &known);

    let mut new_commits: Vec<(Id, String)> = Vec::new();
    for batch in new_shas.chunks(BATCH) {
        new_commits.extend(ingest_batch(app, context, &git, &state, batch)?);
    }
    result.new_commits = count(new_commits.len());
    if !new_commits.is_empty() {
        let new_set: BTreeSet<String> = new_shas.iter().cloned().collect();
        update_branch_membership(app, context, &git, &state, &new_set)?;
    }
    if ingest::needs_reconcile(full, non_fast_forward) {
        let (unreachable, restored) = reconcile_reachability(app, context, &git, &state)?;
        result.marked_unreachable = unreachable;
        result.restored_reachable = restored;
    }
    let new_ids: Vec<Id> = new_commits.iter().map(|(id, _)| *id).collect();
    result.equivalences = detect_equivalences(app, context, &state, &new_ids)?;
    result.links_created = attribute_commits(app, context, &git, &state, &new_ids)?;
    record_sync(
        app,
        context,
        &state,
        &refs,
        &removed,
        &changed,
        non_fast_forward,
        &result,
        &new_ids,
    )?;
    Ok(result)
}

/// `_ingest_batch`: the rows of the given SHAs (oldest first), written in one transaction.
pub(crate) fn ingest_batch(
    app: &mut App,
    context: &RequestContext,
    git: &Git,
    state: &RepoState,
    shas: &[String],
) -> Result<Vec<(Id, String)>, WgError> {
    if shas.is_empty() {
        return Ok(Vec::new());
    }
    let metas = parse_meta(&git.log_meta(shas).map_err(git_error)?)?;
    let by_sha: BTreeMap<String, CommitMeta> = metas
        .into_iter()
        .map(|meta| (meta.sha.clone(), meta))
        .collect();
    let found: Vec<&CommitMeta> = shas.iter().filter_map(|sha| by_sha.get(sha)).collect();
    let lines: Vec<String> = found.iter().map(|meta| diff_line(meta)).collect();
    let diffs: BTreeMap<String, CommitDiff> = if lines.is_empty() {
        BTreeMap::new()
    } else {
        parse_diff_tree(&git.diff_tree_raw_numstat(&lines).map_err(git_error)?)
    };
    let single: Vec<String> = found
        .iter()
        .filter(|meta| meta.parents.len() <= 1)
        .map(|meta| diff_line(meta))
        .collect();
    let patch_ids = if single.is_empty() {
        BTreeMap::new()
    } else {
        let patches = git.diff_tree_patches(&single).map_err(git_error)?;
        if patches.trim().is_empty() {
            BTreeMap::new()
        } else {
            parse_patch_ids(&git.patch_ids(&patches).map_err(git_error)?)
        }
    };
    let repository_id = state.repository.id;
    app.runtime
        .decide(
            context,
            |_session| Ok(()),
            |_: &(), stamp| {
                let mut seq = stamp.ids();
                let mut ingested: Vec<IngestedCommit> = Vec::new();
                for meta in found.iter().copied() {
                    let (next, item) = ingest::ingested_commit(
                        seq,
                        repository_id,
                        meta,
                        diffs.get(&meta.sha),
                        patch_ids.get(&meta.sha).map(String::as_str),
                        stamp.at,
                    );
                    seq = next;
                    ingested.push(item);
                }
                let created: Vec<(Id, String)> = ingested
                    .iter()
                    .map(|item| (item.commit.id, item.commit.sha.clone()))
                    .collect();
                let mut decision = Decision::new(created);
                decision.writes = ingest::insert_rows(&ingested)
                    .into_iter()
                    .map(Write::Insert)
                    .collect();
                Ok(decision)
            },
        )
        .map(|outcome| outcome.reply)
}

/// `_update_branch_membership`: the branches (tracked or local) that contain each new commit.
fn update_branch_membership(
    app: &mut App,
    context: &RequestContext,
    git: &Git,
    state: &RepoState,
    new_shas: &BTreeSet<String>,
) -> Result<(), WgError> {
    let mut branches: Vec<String> = Vec::new();
    for name in state
        .repository
        .tracked_branches
        .iter()
        .cloned()
        .chain(git.local_branches().map_err(git_error)?)
    {
        if !branches.contains(&name) {
            branches.push(name);
        }
    }
    branches.truncate(BRANCH_LIMIT);
    let mut membership: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for branch in &branches {
        let reference = format!("refs/heads/{branch}");
        let output = git
            .run_with(&["rev-list", reference.as_str()], NO_CHECK)
            .map_err(git_error)?;
        if output.returncode != 0 {
            continue;
        }
        for sha in output.stdout.split_whitespace() {
            if new_shas.contains(sha) {
                membership
                    .entry(sha.to_owned())
                    .or_default()
                    .push(branch.clone());
            }
        }
    }
    if membership.is_empty() {
        return Ok(());
    }
    let repository_id = state.repository.id;
    let writes = app.runtime.store.read(|session| {
        membership
            .keys()
            .map(|sha| store::commit_by_sha(session, repository_id, sha))
            .filter_map(|found| found.transpose())
            .collect::<Result<Vec<Commit>, WgError>>()
    })?;
    let updates: Vec<Commit> = writes
        .iter()
        .filter_map(|commit| ingest::with_branches(commit, &membership))
        .collect();
    apply_commit_updates(app, context, updates)
}

/// Writes updated commit rows in one transaction.
fn apply_commit_updates(
    app: &mut App,
    context: &RequestContext,
    updates: Vec<Commit>,
) -> Result<(), WgError> {
    if updates.is_empty() {
        return Ok(());
    }
    app.runtime
        .decide(
            context,
            |_session| Ok(()),
            |_: &(), _stamp| {
                let mut decision = Decision::new(());
                decision.writes = updates
                    .iter()
                    .map(|commit| Write::Update {
                        row: Row::Commit(commit.clone()),
                        expected_version: None,
                    })
                    .collect();
                Ok(decision)
            },
        )
        .map(|_| ())
}

/// `_reconcile_reachability`: marks commits no ref reaches as unreachable, and restores the
/// ones that a ref reaches again. Returns `(marked, restored)`.
fn reconcile_reachability(
    app: &mut App,
    context: &RequestContext,
    git: &Git,
    state: &RepoState,
) -> Result<(i64, i64), WgError> {
    let reachable: BTreeSet<String> = git.rev_list_all().map_err(git_error)?.into_iter().collect();
    let repository_id = state.repository.id;
    let rows = app
        .runtime
        .store
        .read(|session| store::commit_reachability(session, repository_id))?;
    let flips: Vec<(Id, String, bool)> = rows
        .iter()
        .filter(|row| row.reachable != reachable.contains(&row.sha))
        .map(|row| (row.id, row.sha.clone(), row.reachable))
        .collect();
    let availability: BTreeMap<String, bool> = flips
        .iter()
        .filter(|(_, _, was_reachable)| *was_reachable)
        .map(|(_, sha, _)| (sha.clone(), git.object_exists(sha).unwrap_or(false)))
        .collect();
    let ids: Vec<Id> = flips.iter().map(|(id, _, _)| *id).collect();
    let commits = app
        .runtime
        .store
        .read(|session| store::commits_by_ids(session, &ids))?;
    let unreachable = count(flips.iter().filter(|(_, _, was)| *was).count());
    let restored = count(flips.iter().filter(|(_, _, was)| !*was).count());
    app.runtime.decide(
        context,
        |_session| Ok(()),
        |_: &(), stamp| {
            let mut decision = Decision::new(());
            decision.writes = commits
                .iter()
                .filter_map(|commit| {
                    ingest::reachability_change(
                        commit,
                        &reachable,
                        |sha| availability.get(sha).copied().unwrap_or(true),
                        stamp.at,
                    )
                })
                .map(|commit| Write::Update {
                    row: Row::Commit(commit),
                    expected_version: None,
                })
                .collect();
            Ok(decision)
        },
    )?;
    Ok((unreachable, restored))
}

/// `_detect_equivalences`: pairs the new commits with the commits they replace or repeat, and
/// carries the task links of rewritten commits. Returns the number of equivalences written.
pub(crate) fn detect_equivalences(
    app: &mut App,
    context: &RequestContext,
    state: &RepoState,
    new_ids: &[Id],
) -> Result<i64, WgError> {
    if new_ids.is_empty() {
        return Ok(0);
    }
    let repository_id = state.repository.id;
    let plan = app.runtime.store.read(|session| {
        let new_commits = store::commits_by_ids(session, new_ids)?;
        let mut facts: BTreeMap<String, CommitFacts> = BTreeMap::new();
        let mut existing: Vec<(String, String)> = Vec::new();
        for commit in &new_commits {
            facts.insert(commit.id.to_string(), to_facts(commit));
            for other in store::equivalences_of_commit(session, commit.id)? {
                existing.push((
                    other.original_commit_id.to_string(),
                    other.equivalent_commit_id.to_string(),
                ));
            }
            if let Some(patch) = commit.patch_id.as_deref() {
                for other in store::commits_with_patch_id(session, repository_id, patch, commit.id)?
                {
                    facts
                        .entry(other.id.to_string())
                        .or_insert_with(|| to_facts(&other));
                }
            }
            for other in store::unreachable_commits_matching(
                session,
                repository_id,
                &commit.author_email,
                commit.authored_at,
                &commit.subject,
                commit.id,
            )? {
                facts
                    .entry(other.id.to_string())
                    .or_insert_with(|| to_facts(&other));
            }
            if let Some(picked) = cherry_picked_from(&commit.message)
                && let Some(other) = store::commits_by_sha_prefix(session, repository_id, &picked)?
                    .into_iter()
                    .next()
            {
                facts
                    .entry(other.id.to_string())
                    .or_insert_with(|| to_facts(&other));
            }
        }
        let fact_list: Vec<CommitFacts> = facts.into_values().collect();
        let new_strings: Vec<String> = new_ids.iter().map(Id::to_string).collect();
        let equivalences = wg_gitcore::plan_equivalences(&fact_list, &new_strings, &existing);
        let mut planned = Vec::new();
        for equivalence in equivalences {
            let carried = carry_for(session, &equivalence)?;
            planned.push((equivalence, carried));
        }
        Ok(planned)
    })?;
    let count_written = count(plan.len());
    app.runtime
        .decide(
            context,
            |_session| Ok(()),
            |_: &(), stamp| {
                let mut seq = stamp.ids();
                let mut decision = Decision::new(());
                let mut writes = Vec::new();
                for (equivalence, carried) in &plan {
                    let (id, next) = seq.next();
                    seq = next;
                    let equivalent = parse_id(&equivalence.equivalent_id)?;
                    let original = parse_id(&equivalence.original_id)?;
                    writes.push(Write::Insert(Row::CommitEquivalence(CommitEquivalence {
                        id,
                        repository_id,
                        original_commit_id: original,
                        equivalent_commit_id: equivalent,
                        kind: equivalence.kind.as_str().to_owned(),
                        evidence: equivalence_evidence(equivalence),
                        created_at: offset_time(stamp.at, writes.len()),
                    })));
                    for link in carried {
                        let (link_id, next) = seq.next();
                        seq = next;
                        writes.push(Write::Insert(Row::CommitTaskLink(carried_link_row(
                            link_id,
                            equivalent,
                            link,
                            offset_time(stamp.at, writes.len()),
                        ))));
                    }
                }
                decision.writes = writes.into_iter().collect();
                Ok(decision)
            },
        )
        .map(|_| count_written)
}

/// `_attribute`: the links of the new commits, from their trailers, their messages and the
/// managed worktrees of their attempts. Returns the number of links created.
fn attribute_commits(
    app: &mut App,
    context: &RequestContext,
    git: &Git,
    state: &RepoState,
    new_ids: &[Id],
) -> Result<i64, WgError> {
    if new_ids.is_empty() {
        return Ok(0);
    }
    let worktrees = managed_branch_commits(app, git, state)?;
    let workspace_id = state.workspace_id;
    let loaded = app.runtime.store.read(|session| {
        let commits = store::commits_by_ids(session, new_ids)?;
        let world = session.task_world(Some(workspace_id), &[])?;
        let existing = store::links_with_task_keys(session, new_ids)?;
        let mut per_commit: Vec<CommitPlan> = Vec::new();
        for commit in commits {
            let candidates: Vec<(wg_gitcore::Candidate, Option<Id>)> =
                attribution::candidates_of(&commit, &state.prefixes)
                    .into_iter()
                    .map(|candidate| {
                        let task =
                            resolve::task(&world.tasks, Some(workspace_id), &candidate.task_ref)
                                .ok()
                                .map(|task| task.id);
                        (candidate, task)
                    })
                    .collect();
            let links: Vec<CommitTaskLink> = existing
                .iter()
                .filter(|row| row.link.commit_id == commit.id)
                .map(|row| row.link.clone())
                .collect();
            per_commit.push((commit, candidates, links));
        }
        Ok(per_commit)
    })?;
    app.runtime
        .decide(
            context,
            |_session| Ok(()),
            |_: &(), stamp| {
                let mut seq = stamp.ids();
                let mut drafts: Vec<LinkDraft> = Vec::new();
                for (commit, candidates, links) in &loaded {
                    let worktree = worktrees
                        .get(&commit.sha)
                        .map(|(attempt, task, branch)| (*attempt, *task, branch.as_str()));
                    let (next, made) = attribution::attribution_drafts(
                        commit, candidates, links, worktree, seq, stamp.at,
                    );
                    seq = next;
                    drafts.extend(made);
                }
                let created = count(drafts.len());
                let mut decision = Decision::new(created);
                decision.writes = attribution::link_writes(&drafts).into_iter().collect();
                if created > 0 {
                    let (event_id, _) = seq.next();
                    let event = EventDraft::new(
                        stamp,
                        event_id,
                        Some(workspace_id),
                        state.repository.id,
                        EventPayload::GitCommitsAttributed(wg_model::event::GitCommitsAttributed {
                            links_created: created,
                            repository_id: None,
                        }),
                    );
                    decision.events = std::iter::once(event).collect();
                }
                Ok(decision)
            },
        )
        .map(|outcome| outcome.reply)
}

/// `_managed_branch_commits`: for each managed worktree of the repository, the commits between
/// its base and its branch tip, keyed by SHA with the attempt, its task and the branch.
fn managed_branch_commits(
    app: &mut App,
    git: &Git,
    state: &RepoState,
) -> Result<Worktrees, WgError> {
    let repository_id = state.repository.id;
    let rows = app.runtime.store.read(|session| {
        let mut found = Vec::new();
        for row in session.worktrees_with_repository(Some(state.workspace_id), None)? {
            let worktree = row.worktree;
            if worktree.repository_id != repository_id {
                continue;
            }
            let Some(attempt_id) = worktree.attempt_id else {
                continue;
            };
            if let Some(attempt) = session.attempt_by_id(attempt_id)? {
                found.push((
                    worktree.branch,
                    worktree.base_sha,
                    attempt_id,
                    attempt.task_id,
                ));
            }
        }
        Ok(found)
    })?;
    let mut mapping: BTreeMap<String, (Option<Id>, Id, String)> = BTreeMap::new();
    for (branch, base, attempt_id, task_id) in rows {
        let range = format!("{base}..refs/heads/{branch}");
        let output = git
            .run_with(&["rev-list", range.as_str()], NO_CHECK)
            .map_err(git_error)?;
        if output.returncode == 0 {
            for sha in output.stdout.split_whitespace() {
                mapping.entry(sha.to_owned()).or_insert((
                    Some(attempt_id),
                    task_id,
                    branch.clone(),
                ));
            }
        }
    }
    Ok(mapping)
}

/// The writes of a sync that are not commit rows: the refs, the repository's scan time and
/// shallow flag, and the `git.*` events.
#[allow(clippy::too_many_arguments)]
fn record_sync(
    app: &mut App,
    context: &RequestContext,
    state: &RepoState,
    refs: &BTreeMap<String, String>,
    removed: &[String],
    changed: &[String],
    non_fast_forward: bool,
    result: &SyncResult,
    new_ids: &[Id],
) -> Result<(), WgError> {
    let repository_id = state.repository.id;
    let workspace_id = state.workspace_id;
    let shallow = result.shallow;
    let had_refs = !state.known_refs.is_empty();
    let (repository, existing_refs, links, new_commits) = app.runtime.store.read(|session| {
        let repository = session
            .fetch::<Repository>(&RowKey::Repository(repository_id))?
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "repository not found"))?;
        let existing: Vec<RepositoryRef> = session.refs(repository_id)?;
        let links = store::links_with_task_keys(session, new_ids)?;
        let commits = store::commits_by_ids(session, new_ids)?;
        Ok((repository, existing, links, commits))
    })?;
    let removed_sorted: Vec<String> = {
        let mut names = removed.to_vec();
        names.sort();
        names
    };
    let changed_sorted: Vec<String> = {
        let mut names = changed.to_vec();
        names.sort();
        names
    };
    let commit_events = new_ids.len() <= ingest::PER_COMMIT_EVENT_LIMIT && had_refs;
    app.runtime
        .decide(
            context,
            |_session| Ok(()),
            |_: &(), stamp| {
                let mut seq = stamp.ids();
                let mut decision = Decision::new(());
                let mut writes: Vec<Write> = Vec::new();
                for (name, sha) in refs {
                    match existing_refs.iter().find(|row| &row.name == name) {
                        Some(row) => writes.push(Write::Update {
                            row: Row::RepositoryRef(RepositoryRef {
                                sha: sha.clone(),
                                updated_at: stamp.at,
                                ..row.clone()
                            }),
                            expected_version: None,
                        }),
                        None => {
                            let (id, next) = seq.next();
                            seq = next;
                            writes.push(Write::Insert(Row::RepositoryRef(RepositoryRef {
                                id,
                                repository_id,
                                name: name.clone(),
                                sha: sha.clone(),
                                updated_at: stamp.at,
                            })));
                        }
                    }
                }
                for name in removed {
                    if let Some(row) = existing_refs.iter().find(|row| &row.name == name) {
                        writes.push(Write::Delete(RowKey::RepositoryRef(row.id)));
                    }
                }
                let mut health = repository.health_detail.clone();
                if shallow {
                    health.insert("shallow".to_owned(), serde_json::Value::Bool(true));
                }
                writes.push(Write::Update {
                    row: Row::Repository(Repository {
                        last_scanned_at: Some(stamp.at),
                        is_shallow: shallow,
                        health_detail: health,
                        ..repository.clone()
                    }),
                    expected_version: None,
                });
                decision.writes = writes.into_iter().collect();

                let (summary_id, next) = seq.next();
                seq = next;
                let mut events = vec![EventDraft::new(
                    stamp,
                    summary_id,
                    Some(workspace_id),
                    repository_id,
                    EventPayload::GitSyncCompleted(result.completed_payload(repository_id)),
                )];
                if result.refs_changed > 0 {
                    let (id, next) = seq.next();
                    seq = next;
                    events.push(
                        EventDraft::new(
                            stamp,
                            id,
                            Some(workspace_id),
                            repository_id,
                            EventPayload::RepositoryChangesDetected(RepositoryChangesDetected {
                                changed_refs: changed_sorted
                                    .iter()
                                    .take(REF_LIST_LIMIT)
                                    .cloned()
                                    .collect(),
                                removed_refs: removed_sorted
                                    .iter()
                                    .take(REF_LIST_LIMIT)
                                    .cloned()
                                    .collect(),
                                non_fast_forward,
                            }),
                        )
                        .caused_by(summary_id),
                    );
                }
                if commit_events {
                    for commit in &new_commits {
                        let (id, next) = seq.next();
                        seq = next;
                        let task_ids: Vec<Id> = links
                            .iter()
                            .filter(|row| row.link.commit_id == commit.id)
                            .map(|row| row.link.task_id)
                            .collect();
                        events.push(
                            EventDraft::new(
                                stamp,
                                id,
                                Some(workspace_id),
                                commit.id,
                                EventPayload::GitCommitIngested(ingest::commit_ingested_payload(
                                    commit,
                                    repository_id,
                                    &task_ids,
                                )),
                            )
                            .caused_by(summary_id),
                        );
                    }
                }
                decision.events = events.into_iter().collect();
                Ok(decision)
            },
        )
        .map(|_| ())
}

/// The task links a rewritten commit inherits from its original (`_carry_links`).
fn carry_for(
    session: &mut wg_store::Session<'_>,
    equivalence: &Equivalence,
) -> Result<Vec<CarriedLink>, WgError> {
    if equivalence.kind != wg_gitcore::EquivalenceKind::Rewrite {
        return Ok(Vec::new());
    }
    let original = parse_id(&equivalence.original_id)?;
    let rewritten = parse_id(&equivalence.equivalent_id)?;
    let original_links: Vec<OriginalLink> = store::links_with_task_keys(session, &[original])?
        .into_iter()
        .map(|row| OriginalLink {
            task_id: row.link.task_id.to_string(),
            source: row.link.source.clone(),
            confidence: row.link.confidence.0,
            attempt_id: row.link.attempt_id.map(|id| id.to_string()),
        })
        .collect();
    let rewritten_tasks: Vec<String> = store::links_with_task_keys(session, &[rewritten])?
        .into_iter()
        .filter(|row| row.link.source == "rewrite_reconciliation")
        .map(|row| row.link.task_id.to_string())
        .collect();
    Ok(wg_gitcore::carry_links(
        &equivalence.original_sha,
        &original_links,
        &rewritten_tasks,
    ))
}
