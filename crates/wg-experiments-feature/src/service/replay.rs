//! Shell: the git-walking glue of `src/workgraph/experiments/replay.py` and the replay pool of
//! `pool.py` (IF-20, C075, C076). Each function runs git, hands the text to the pure functions in
//! [`crate::spec::replay`] and `wg_experiments`, and acts on what they decide.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;
use wg_experiments::pool::{self, ExistingReplay, Step};
use wg_experiments::replay::{self, ReplayConfig};
use wg_gitcore::RunOptions;
use wg_json::Value;
use wg_model::py_repr_str;
use wg_registry::checkout::primary_checkout;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext, draw};
use wg_types::{ErrorCode, Id, Permission, WgError};
use wg_wire::convention::json_value;

use crate::service::exec::{
    config_for, environment, exit_code, out, out_unchecked, resolve_commit, run, scratch_dir,
    show_file,
};
use crate::spec::replay as shape;
use crate::spec::replay::{Candidate, Extras, Validation, WorktreeChanges, validate_ref};
use crate::spec::runner::{timeout_ms, tree_files};

/// `_changes`: the diff of `sha` against its first parent; a root commit is shown against the
/// empty tree.
fn changes(
    repo: &Path,
    sha: &str,
    parent: Option<&str>,
    flags: &[&str],
) -> Result<String, WgError> {
    let args: Vec<&str> = match parent {
        None => ["show", "--format="]
            .into_iter()
            .chain(flags.iter().copied())
            .chain([sha])
            .collect(),
        Some(parent) => ["diff"]
            .into_iter()
            .chain(flags.iter().copied())
            .chain([parent, sha])
            .collect(),
    };
    out(repo, &args)
}

/// `_tree_files`: every path of the tree of `commit`.
///
/// # Errors
///
/// Returns the `GitError` of a failed `ls-tree`.
pub fn tree_files_at(repo: &Path, commit: &str) -> Result<BTreeSet<String>, WgError> {
    Ok(tree_files(&out(
        repo,
        &["ls-tree", "-r", "-z", "--name-only", commit],
    )?))
}

/// `template_inputs`: the existing test paths at `sha` and their packages, as the oracle
/// template expects them.
///
/// # Errors
///
/// Returns the `GitError` of a failed git call.
pub fn template_inputs(
    repo: &Path,
    sha: &str,
    test_paths: &[String],
) -> Result<(Vec<String>, Vec<String>), WgError> {
    let commit = resolve_commit(repo, sha)?;
    Ok(replay::template_inputs(
        &tree_files_at(repo, &commit)?,
        test_paths,
    ))
}

/// `analyze_commit`: measures one commit as a replay task.
///
/// # Errors
///
/// Returns the `GitError` of a failed git call.
pub fn analyze_commit(repo: &Path, sha: &str, config: &ReplayConfig) -> Result<Candidate, WgError> {
    let commit = resolve_commit(repo, sha)?;
    let header = shape::parse_header(&out(
        repo,
        &["show", "-s", "--format=%P%x00%s%x00%b", &commit],
    )?)?;
    let parent = header.parent();
    let files = tree_files_at(repo, &commit)?;
    let measured = shape::measure(
        &changes(repo, &commit, parent, &["--numstat", "-z", "-M"])?,
        &files,
        config,
    );
    let patch = shape::needs_patch(&measured)
        .then(|| changes(repo, &commit, parent, &["-M", "-U0"]))
        .transpose()?;
    let blind = shape::needs_whitespace_blind(&header, &measured)
        .then(|| changes(repo, &commit, parent, &["--numstat", "-z", "-M", "-w"]))
        .transpose()?;
    let tests = shape::test_paths(&measured);
    let names: BTreeMap<String, Option<String>> = shape::package_dirs(&tests, &files)
        .into_iter()
        .map(|dir| {
            let name = show_file(repo, &commit, &shape::manifest_path(&dir))
                .and_then(|data| shape::package_name(&data));
            (dir, name)
        })
        .collect();
    let oracle = replay::resolve_oracle(config, &tests, &files, |dir| {
        names.get(dir).cloned().flatten()
    });
    Ok(shape::candidate(
        &commit,
        &header,
        &measured,
        &Extras {
            patch: patch.as_deref(),
            blind_numstat: blind.as_deref(),
        },
        oracle,
        config,
    ))
}

/// What `replay scan` filters on.
pub struct ScanOptions<'a> {
    pub rev: &'a str,
    pub limit: Option<i64>,
    pub min_lines: i64,
    pub max_lines: Option<i64>,
    pub include_excluded: bool,
}

/// `scan`: candidates from `rev` (newest first) whose source size is in range. Read-only.
///
/// # Errors
///
/// Returns the `GitError` of an invalid ref or a failed git call.
pub fn scan(
    repo: &Path,
    options: &ScanOptions<'_>,
    config: &ReplayConfig,
) -> Result<Vec<Candidate>, WgError> {
    validate_ref(options.rev)?;
    let listing = out(repo, &["rev-list", "--no-merges", options.rev])?;
    let limit = options
        .limit
        .map(|limit| usize::try_from(limit).unwrap_or(0));
    let mut candidates: Vec<Candidate> = Vec::new();
    for sha in listing.split_whitespace() {
        if limit.is_some_and(|limit| candidates.len() >= limit) {
            break;
        }
        let candidate = analyze_commit(repo, sha, config)?;
        let wanted = replay::in_size_range(
            candidate.features.src_lines,
            options.min_lines,
            options.max_lines,
        ) && (candidate.excluded.is_empty() || options.include_excluded);
        if wanted {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

/// `_sync_path`: makes the index entry for `path` equal to its content at `sha`, or absent if it
/// has none.
fn sync_path(repo: &Path, index: &str, sha: &str, path: &str) -> Result<(), WgError> {
    let env = [("GIT_INDEX_FILE", index)];
    let options = RunOptions {
        env: &env,
        ..RunOptions::CHECKED
    };
    let entry = out(repo, &["ls-tree", "-z", sha, "--", path])?;
    match shape::tree_entry(&entry) {
        None => run(repo, &["update-index", "--force-remove", path], options),
        Some((mode, blob)) => run(
            repo,
            &["update-index", "--add", "--cacheinfo", &mode, &blob, path],
            options,
        ),
    }
    .map(|_| ())
}

/// `create_base`: creates `wg/replay/<sha[:12]>`, the parent's tree with the commit's test
/// changes applied, on a temporary index so the working tree and the repository's own index are
/// untouched.
///
/// # Errors
///
/// Returns the `GitError` of a failed git call, and an error for a root commit or a branch that
/// exists with a different tree.
pub fn create_base(repo: &Path, candidate: &Candidate) -> Result<String, WgError> {
    let parent = candidate.parent.as_deref().ok_or_else(|| {
        WgError::new(
            ErrorCode::WorkgraphError,
            format!("{} has no parent, so it has no replay base", candidate.sha),
        )
    })?;
    let config = config_for(repo);
    let branch = shape::base_branch(&candidate.sha);
    let reference = format!("refs/heads/{branch}");
    let scratch = scratch_dir("wg-replay-index-")?;
    let built = build_tree(repo, &scratch, parent, &candidate.sha, &config);
    let _ = std::fs::remove_dir_all(&scratch);
    let tree = built?;
    let probe = format!("{reference}^{{tree}}");
    let existing = out_unchecked(repo, &["rev-parse", "--verify", "--quiet", &probe])?;
    let existing = existing.trim();
    if !existing.is_empty() {
        return if existing == tree {
            Ok(branch)
        } else {
            Err(WgError::new(
                ErrorCode::WorkgraphError,
                format!("branch {branch} already exists with a different tree"),
            ))
        };
    }
    let commit = out(
        repo,
        &[
            "commit-tree",
            &tree,
            "-p",
            parent,
            "-m",
            shape::BASE_MESSAGE,
        ],
    )?;
    out(repo, &["update-ref", &reference, commit.trim()])?;
    Ok(branch)
}

/// The tree of the parent with the commit's test changes, written through a temporary index.
fn build_tree(
    repo: &Path,
    scratch: &Path,
    parent: &str,
    sha: &str,
    config: &ReplayConfig,
) -> Result<String, WgError> {
    let index = scratch.join("index").display().to_string();
    let env = [("GIT_INDEX_FILE", index.as_str())];
    let with_index = RunOptions {
        env: &env,
        ..RunOptions::CHECKED
    };
    run(repo, &["read-tree", parent], with_index)?;
    let listing = out(repo, &["diff", "--name-status", "-z", "-M", parent, sha])?;
    for path in shape::base_sync_paths(&replay::parse_name_status(&listing), config) {
        sync_path(repo, &index, sha, &path)?;
    }
    let tree = run(repo, &["write-tree"], with_index)?;
    Ok(tree.stdout.trim().to_owned())
}

/// `_run_oracle`: the exit code of each command in a detached worktree of `reference`.
fn run_oracle(
    repo: &Path,
    reference: &str,
    path: &Path,
    oracle: &[Vec<String>],
    timeout: u64,
    env: &BTreeMap<OsString, OsString>,
) -> Result<Vec<i64>, WgError> {
    let shown = path.display().to_string();
    out(repo, &["worktree", "add", "--detach", &shown, reference])?;
    let codes: Vec<i64> = oracle
        .iter()
        .map(|command| exit_code(command, path, timeout, env))
        .collect();
    let _ = run(
        repo,
        &["worktree", "remove", "--force", &shown],
        RunOptions::UNCHECKED,
    );
    let _ = std::fs::remove_dir_all(path);
    Ok(codes)
}

/// `validate`: the oracle must fail at the base (some command non-zero) and pass at the real
/// commit (all zero). `env` is added to the inherited environment of every oracle command.
///
/// # Errors
///
/// Returns the `GitError` of an invalid ref or a failed worktree.
pub fn validate(
    repo: &Path,
    candidate: &Candidate,
    base_ref: &str,
    timeout_seconds: f64,
    extra_env: &[(String, String)],
) -> Result<Validation, WgError> {
    if candidate.oracle.is_empty() {
        return Ok(Validation::rejected("no_oracle", Vec::new(), Vec::new()));
    }
    validate_ref(base_ref)?;
    let env = environment(extra_env);
    let timeout = timeout_ms(timeout_seconds);
    let root = scratch_dir("wg-replay-")?;
    // The solution runs first: commits that were red when made are the commonest rejection, and
    // checking them first skips the base run (a full build in a compiled language).
    let verdict = run_oracle(
        repo,
        &candidate.sha,
        &root.join("solution"),
        &candidate.oracle,
        timeout,
        &env,
    )
    .and_then(|solution| match shape::solution_verdict(&solution) {
        Some(rejected) => Ok(rejected),
        None => run_oracle(
            repo,
            base_ref,
            &root.join("base"),
            &candidate.oracle,
            timeout,
            &env,
        )
        .map(|base| shape::final_verdict(base, solution)),
    });
    let _ = std::fs::remove_dir_all(&root);
    verdict
}

/// `worktree_changes`: measures the worktree's current contents against `base_sha`, on a
/// temporary index so the worktree's real index and files are never written.
///
/// # Errors
///
/// Returns the `GitError` of a failed git call.
pub fn worktree_changes(worktree: &Path, base_sha: &str) -> Result<WorktreeChanges, WgError> {
    let config = config_for(worktree);
    let base = resolve_commit(worktree, base_sha)?;
    let scratch = scratch_dir("wg-worktree-index-")?;
    let measured = measure_index(worktree, &scratch, &base, &config);
    let _ = std::fs::remove_dir_all(&scratch);
    measured
}

fn measure_index(
    worktree: &Path,
    scratch: &Path,
    base: &str,
    config: &ReplayConfig,
) -> Result<WorktreeChanges, WgError> {
    let index = scratch.join("index").display().to_string();
    let env = [("GIT_INDEX_FILE", index.as_str())];
    let options = RunOptions {
        env: &env,
        ..RunOptions::CHECKED
    };
    let staged = |args: &[&str]| run(worktree, args, options).map(|output| output.stdout);
    staged(&["read-tree", base])?;
    staged(&["add", "-A"])?;
    let files = tree_files(&staged(&["ls-files", "-z"])?);
    let numstat = staged(&["diff", "--cached", "-M", "--numstat", "-z", base])?;
    let patch = staged(&["diff", "--cached", "-M", "-U0", base])?;
    let name_status = staged(&["diff", "--cached", "-M", "--name-status", "-z", base])?;
    Ok(shape::worktree_changes(
        &numstat,
        &patch,
        &name_status,
        &files,
        config,
    ))
}

/// `ReplayPool._checkout`: the primary checkout of the repository named in the workspace.
fn checkout_of(app: &mut App, scope: Id, repository: &str) -> Result<PathBuf, WgError> {
    app.runtime.query(
        |session| {
            let slice = session.registry_world(Some(scope))?;
            let found = resolve::repository(&slice, scope, repository, false)?.clone();
            let checkouts = session.checkouts(found.id)?;
            Ok((found, checkouts))
        },
        |(found, checkouts)| {
            primary_checkout(checkouts, true)
                .map(|checkout| PathBuf::from(&checkout.path))
                .ok_or_else(|| {
                    WgError::new(
                        ErrorCode::ValidationFailed,
                        format!(
                            "repository {} has no available checkout",
                            py_repr_str(&found.name)
                        ),
                    )
                })
        },
    )
}

/// `ReplayPool.scan`.
///
/// # Errors
///
/// Returns the `NotFound` of an unknown repository and the errors of the walk.
pub fn replay_scan(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    repository: &str,
    options: &ScanOptions<'_>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_default(workspace)?;
    let path = checkout_of(app, scope, repository)?;
    let candidates = scan(&path, options, &config_for(&path))?;
    Ok(Value::List(
        candidates
            .iter()
            .map(|candidate| json_value(&candidate.as_json()))
            .collect(),
    ))
}

/// `_replay_tasks_by_sha`: the commit-to-task index of the workspace's replay tasks.
fn existing_replays(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
) -> Result<pool::ExistingIndex, WgError> {
    let mut found: Vec<(String, Option<String>, String)> = Vec::new();
    let mut offset = 0_i64;
    loop {
        let body = serde_json::json!({
            "tags": [pool::REPLAY_TAG],
            "limit": 500,
            "offset": offset,
            "include_archived": true,
        });
        let page = wg_tasks_feature::service::task_list(app, context, workspace, &body)?;
        found.extend(shape::existing_of(&page));
        match shape::next_offset(&page) {
            Some(next) => offset = next,
            None => break,
        }
    }
    let tasks: Vec<ExistingReplay<'_>> = found
        .iter()
        .map(|(key, sha, classifier)| ExistingReplay {
            key,
            sha: sha.as_deref(),
            classifier,
        })
        .collect();
    Ok(pool::existing_index(&tasks))
}

/// What `replay create` is told besides the commits.
pub struct CreateOptions {
    pub validate: bool,
    pub oracle_template: Option<Vec<String>>,
    pub env: Vec<(String, String)>,
    pub timeout: f64,
}

/// `ReplayPool.create`: one replay task per commit. Existing replay tasks for a commit are
/// reported, not duplicated.
///
/// # Errors
///
/// Returns the errors of the checkout, the walk and the task creation.
pub fn replay_create(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    repository: &str,
    shas: &[String],
    options: &CreateOptions,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::ExperimentWrite)?;
    let scope = app.workspace_or_default(workspace)?;
    let path = checkout_of(app, scope, repository)?;
    let config = ReplayConfig {
        oracle_template: options.oracle_template.clone(),
        ..config_for(&path)
    };
    let mut index = existing_replays(app, context, workspace)?;
    let mut results: Vec<Json> = Vec::new();
    for sha in shas {
        let candidate = analyze_commit(&path, sha, &config)?;
        let excluded: Vec<&str> = candidate.excluded.iter().map(String::as_str).collect();
        match pool::step(&excluded, &candidate.sha, &index) {
            Step::Skipped { reason } => {
                results.push(shape::result_json(
                    &candidate.sha,
                    "skipped",
                    None,
                    Some(&reason),
                ));
            }
            Step::Exists { task_key } => {
                results.push(shape::result_json(
                    &candidate.sha,
                    "exists",
                    Some(&task_key),
                    None,
                ));
            }
            Step::Create => {
                let base_ref = create_base(&path, &candidate)?;
                let validation = if options.validate {
                    Some(validate(
                        &path,
                        &candidate,
                        &base_ref,
                        options.timeout,
                        &options.env,
                    )?)
                } else {
                    None
                };
                if let Some(rejected) = validation.as_ref().filter(|checked| !checked.ok) {
                    results.push(shape::result_json(
                        &candidate.sha,
                        "rejected",
                        None,
                        rejected.reason.as_deref(),
                    ));
                    continue;
                }
                let (at, _) = draw()?;
                let validated_at = at.format_isoformat();
                let body = shape::task_body(
                    &candidate,
                    repository,
                    &base_ref,
                    validation
                        .as_ref()
                        .map(|checked| (checked, validated_at.as_str())),
                );
                let task = wg_tasks_feature::service::task_create(app, context, workspace, &body)?;
                let key = wg_clispec::args::s(wg_clispec::args::get(&task, "key"));
                index = index.insert(candidate.sha.clone(), key.clone());
                results.push(shape::result_json(
                    &candidate.sha,
                    "created",
                    Some(&key),
                    None,
                ));
            }
        }
    }
    Ok(Value::List(results.iter().map(json_value).collect()))
}
