//! `attempt submit` (`RunService.submit_external` and `conclude_attempt`): the outcome of an
//! external attempt's worktrees is captured and its commits linked, the read-only policy is
//! applied, and the attempt is either failed (with its task) or handed to review.
//!
//! Verification runs and stack gates are not ported: a submit that would need either is refused
//! before anything is written.

use serde_json::{Value as Json, json};
use wg_exec::attempt::submit_check;
use wg_exec::manifest::{ManifestEntry, RepoObservation, needs_commit_range, plan_capture};
use wg_exec::run::{CaptureFacts, Conclusion, conclude, task_reason};
use wg_git::Git;
use wg_json::Value;
use wg_model::entity::AgentAttempt;
use wg_model::{Task, Write};
use wg_runtime::App;
use wg_runtimespec::RequestContext;
use wg_runtimespec::effects::GIT_TIMEOUT_MS;
use wg_store::Session;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::accept::active_item;
use wg_types::{AttemptStatus, ErrorCode, Id, Permission, TaskStatus, WgError};
use wg_verify::applicable_checks;
use wg_wire::convention::json_value;

use super::attempt_runs::{AttemptWorld, DecideWritten, load_attempt_world};
use super::resolve::find_attempt;
use super::worktrees::worktree_release;
use crate::spec::attempt_capture::{
    CaptureRows, Captured, capture_decision, outcome_object, violations_text,
};
use crate::spec::attempt_writes::{
    Finish, attempt_limit, escalated, failures, finished, gone, not_ported, start_chain, task_moved,
};

/// `WorktreeManager.capture_outcome`: the outcome manifest of an attempt, its modified
/// repositories and telemetry, and the change set it belongs to.
pub fn capture_outcome(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
) -> Result<Captured, WgError> {
    let (attempt, entries, change_set) = app.runtime.store.read(|session| {
        let attempt = session.attempt_by_id(attempt_id)?;
        let manifests = session.manifests_of_attempt(attempt_id)?;
        let baseline = attempt
            .as_ref()
            .and_then(|row| row.baseline_manifest_id)
            .and_then(|wanted| manifests.into_iter().find(|row| row.id == wanted));
        let (Some(attempt), Some(baseline)) = (attempt, baseline) else {
            return Err(WgError::new(
                ErrorCode::NotFound,
                format!("attempt {attempt_id} has no prepared workspace"),
            ));
        };
        let entries: Vec<ManifestEntry> = baseline
            .entries
            .iter()
            .filter_map(|entry| ManifestEntry::from_json(&Json::Object(entry.clone())))
            .collect();
        let change_set = match session
            .change_sets_of_attempt(attempt_id)?
            .into_iter()
            .next()
        {
            Some(set) => {
                let items = session.change_set_items(set.id)?;
                Some((set, items))
            }
            None => None,
        };
        Ok((attempt, entries, change_set))
    })?;
    // What git says about each checkout of the baseline.
    let git_failed = |err: wg_git::GitError| WgError::new(ErrorCode::GitError, err.to_string());
    let observations = entries
        .iter()
        .map(|entry| {
            let git = Git {
                path: std::path::PathBuf::from(&entry.checkout_path),
                timeout_ms: GIT_TIMEOUT_MS,
            };
            let head = if git.is_repository().map_err(git_failed)? {
                git.head().map_err(git_failed)?
            } else {
                None
            };
            let changed_files = match head {
                Some(_) => {
                    let status = git.status_porcelain().map_err(git_failed)?;
                    status
                        .staged
                        .into_iter()
                        .chain(status.unstaged)
                        .chain(status.untracked)
                        .collect()
                }
                None => Vec::new(),
            };
            let commits = match (&entry.sha, &head) {
                (Some(sha), Some(tip)) if needs_commit_range(Some(sha), Some(tip)) => {
                    git.rev_list_range(sha, tip).map_err(git_failed)?
                }
                _ => Vec::new(),
            };
            Ok(RepoObservation {
                head,
                changed_files,
                commits,
            })
        })
        .collect::<Result<Vec<_>, WgError>>()?;
    let plan = plan_capture(&entries, &observations, change_set.is_some());
    let commits: Vec<(String, Vec<String>)> = entries
        .iter()
        .zip(&observations)
        .map(|(entry, seen)| {
            let listed = if needs_commit_range(entry.sha.as_deref(), seen.head.as_deref()) {
                seen.commits.clone()
            } else {
                Vec::new()
            };
            (entry.repository_id.clone(), listed)
        })
        .collect();
    let rows = CaptureRows {
        attempt: &attempt,
        change_set: change_set
            .as_ref()
            .map(|(set, items)| (set, items.as_slice())),
    };
    Ok(app
        .runtime
        .decide(
            context,
            |_session| Ok(()),
            |_: &(), stamp| Ok(capture_decision(&rows, &plan, &commits, stamp)),
        )?
        .reply)
}

/// The facts a submit checks before it writes anything.
struct SubmitFacts {
    attempt: AgentAttempt,
    task: Task,
    stacked: bool,
    has_checks: bool,
}

fn load_submit(session: &mut Session<'_>, reference: &str) -> Result<SubmitFacts, WgError> {
    let attempt = find_attempt(session, reference)?;
    let world: TaskWorld = session.task_world(Some(attempt.workspace_id), &[])?;
    let task = world.tasks.task(attempt.task_id).cloned().ok_or_else(|| {
        WgError::new(
            ErrorCode::NotFound,
            format!("task {} not found", attempt.task_id),
        )
    })?;
    let configs = session.verification_configs(task.workspace_id)?;
    let scope = wg_tasks_feature::spec::claims::scope_of(&world.tasks, &task);
    Ok(SubmitFacts {
        stacked: active_item(&world.stacks, task.id).is_some(),
        has_checks: !applicable_checks(&configs, &scope)?.is_empty(),
        attempt,
        task,
    })
}

/// `RunService.submit_external`: concludes an external managed attempt from the commits the agent
/// made in its worktrees. The reply is the conclusion plus the task and attempt statuses after.
pub fn attempt_submit(
    app: &mut App,
    context: &RequestContext,
    reference: &str,
    failed: bool,
    summary: Option<&str>,
    error: Option<&str>,
) -> Result<Value, WgError> {
    app.require_permission(context, Permission::TaskClaim)?;
    app.require_permission(context, Permission::AttemptWrite)?;
    app.require_permission(context, Permission::VerifyRun)?;
    let status = if failed { "failed" } else { "succeeded" };
    let facts = app
        .runtime
        .store
        .read(|session| load_submit(session, reference))?;
    let attempt_id = facts.attempt.id;
    submit_check(
        status,
        &attempt_id.to_string(),
        facts.attempt.status,
        facts.attempt.baseline_manifest_id.is_some(),
        &facts.task.key.to_string(),
        facts.task.status,
    )?;
    if facts.stacked {
        app.require_permission(context, Permission::StackWrite)?;
        if !failed {
            return Err(not_ported("submitting a stack child"));
        }
    }
    if !failed && facts.has_checks {
        return Err(not_ported("running verification checks on submit"));
    }
    let attempted = RequestContext {
        attempt: Some(attempt_id),
        ..context.clone()
    };
    let context = &attempted;
    let task_id = facts.task.id;
    // Without a captured outcome the attempt's commits are unknown, so it cannot count as a
    // success.
    let (captured, capture_error) = match capture_outcome(app, context, attempt_id) {
        Ok(captured) => (Some(captured), None),
        Err(err) => (None, Some(err.message)),
    };
    let link_error = match capture_error {
        None => wg_git_feature::service::link_attempt_commits(app, context, attempt_id)
            .err()
            .map(|err| err.message),
        Some(_) => None,
    };
    let violations = captured
        .as_ref()
        .and_then(|found| violations_text(&found.violations));
    let conclusion = conclude(
        status,
        error,
        &CaptureFacts {
            violations,
            capture_error: capture_error.clone(),
        },
    )?;
    let limit = attempt_limit(&app.config()?);
    let reply = match conclusion {
        Conclusion::Failed {
            attempt: ended,
            exit_reason,
            error: text,
        } => {
            app.runtime.decide_written(
                context,
                |session| load_attempt_world(session, attempt_id),
                |loaded, stamp| {
                    let chain = start_chain(stamp, &loaded.world.tasks);
                    let (chain, row) = finished(
                        chain,
                        stamp,
                        &loaded.attempt,
                        ended,
                        Finish {
                            exit_reason: Some(exit_reason.clone()),
                            error: Some(text.clone()),
                            summary: summary.map(str::to_owned),
                            ..Finish::default()
                        },
                    )?;
                    let current = loaded
                        .world
                        .tasks
                        .task(task_id)
                        .ok_or_else(|| gone(task_id))?;
                    let chain =
                        match wg_exec::attempt::task_after_attempt_end(current.status, ended) {
                            Some(target) => task_moved(
                                chain,
                                &loaded.world.stacks,
                                stamp,
                                task_id,
                                target,
                                &task_reason(row.number, ended),
                            )?,
                            None => chain,
                        };
                    let chain = escalated(
                        chain,
                        stamp,
                        task_id,
                        failures(&loaded.attempts, &row),
                        limit,
                    );
                    Ok(chain.decision)
                },
            )?;
            worktree_release(app, context, attempt_id, false, false)?;
            json!({"status": status, "error": error})
        }
        Conclusion::Succeeded => {
            conclude_success(app, context, attempt_id, summary)?;
            worktree_release(app, context, attempt_id, false, false)?;
            let mut outcome = captured.as_ref().map(outcome_object).unwrap_or(Json::Null);
            if let (Some(link), Some(map)) = (&link_error, outcome.as_object_mut()) {
                map.insert("link_error".to_owned(), json!(link));
            }
            json!({"status": "succeeded", "verification": Json::Null, "outcome": outcome})
        }
    };
    let (task_status, attempt_status) = app.runtime.store.read(|session| {
        let loaded = load_attempt_world(session, attempt_id)?;
        let task = loaded
            .world
            .tasks
            .task(task_id)
            .map(|task| task.status)
            .ok_or_else(|| gone(task_id))?;
        Ok((task, loaded.attempt.status))
    })?;
    let merged = match reply {
        Json::Object(mut map) => {
            map.insert("task_status".to_owned(), json!(task_status.as_str()));
            map.insert("attempt_status".to_owned(), json!(attempt_status.as_str()));
            Json::Object(map)
        }
        other => other,
    };
    Ok(json_value(&merged))
}

/// The success half of `conclude_attempt`: the agent's success stands, so the task is handed to
/// review and the attempt succeeds.
fn conclude_success(
    app: &mut App,
    context: &RequestContext,
    attempt_id: Id,
    summary: Option<&str>,
) -> Result<(), WgError> {
    app.runtime.decide_written(
        context,
        |session| {
            let loaded = load_attempt_world(session, attempt_id)?;
            let configs = session.verification_configs(loaded.attempt.workspace_id)?;
            Ok((loaded, configs))
        },
        |(loaded, configs): &(AttemptWorld, Vec<wg_model::entity::VerificationConfig>), stamp| {
            let AttemptWorld {
                attempt,
                world,
                attempts: _,
            } = loaded;
            let task = world
                .tasks
                .task(attempt.task_id)
                .ok_or_else(|| gone(attempt.task_id))?;
            // The summary is kept, the exit code and duration are cleared.
            let recorded = AgentAttempt {
                summary: match summary.filter(|text| !text.is_empty()) {
                    Some(text) => Some(text.chars().take(50_000).collect()),
                    None => attempt.summary.clone(),
                },
                exit_code: None,
                duration_ms: None,
                updated_at: stamp.at,
                ..attempt.clone()
            };
            let chain = start_chain(stamp, &world.tasks);
            let chain = wg_tasks::transition::Applied {
                decision: chain.decision.write(Write::update(recorded.clone())),
                ..chain
            };
            let reported = wg_tasks_feature::spec::claims::Reported {
                world: world.clone(),
                configs: configs.clone(),
                attempt: Some(recorded.clone()),
            };
            let chain = if matches!(task.status, TaskStatus::Running | TaskStatus::Claimed) {
                wg_tasks_feature::spec::claims::complete_in(
                    chain,
                    &reported,
                    stamp,
                    task,
                    summary.unwrap_or_default(),
                    Some(&attempt_id.to_string()),
                )?
            } else {
                chain
            };
            let after = chain
                .tasks
                .task(task.id)
                .map_or(task.status, |row| row.status);
            let chain = if after == TaskStatus::AwaitingVerification {
                chain
            } else {
                let (chain, _) = finished(
                    chain,
                    stamp,
                    &recorded,
                    AttemptStatus::Succeeded,
                    Finish {
                        exit_reason: Some("agent_succeeded".to_owned()),
                        ..Finish::default()
                    },
                )?;
                chain
            };
            Ok(chain.decision)
        },
    )?;
    Ok(())
}
