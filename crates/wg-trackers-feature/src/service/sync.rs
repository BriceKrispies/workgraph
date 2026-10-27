//! Shell: the sync of a tracker connection (`TrackerSyncService.sync`, `_apply_page`, `_reconcile`
//! and `_finish` in `integrations/trackers/sync.py`), run now by `tracker sync` or as the
//! `tracker_sync` job. The issues come from an [`IssueSource`]: the Jira client in production, a
//! fake in tests. The sync rules are the `wg-trackers` plans and the page writes are
//! [`crate::spec::sync`]; this module does the fetching, the reads and the transactions.
//!
//! Fetching happens outside any write transaction. Each page of up to [`PAGE_SIZE`] issues is then
//! applied in one transaction as `system:tracker-sync`.
//!
//! The listing is read to the end before the first page is applied. Python applies pages while
//! the listing is still being read, so a failure part-way through a listing leaves Python with the
//! pages before it applied and this port with none; the next sync reaches the same state either way.

use std::collections::HashSet;

use serde_json::Value as Json;
use wg_model::slice::QueueSlice;
use wg_net::{Flavour, JiraClient, Net};
use wg_netspec::TrackerError;
use wg_runtime::{
    App, HandlerError, HandlerResult, JobContext, Queue, RequestContext, index_written,
    latest_succeeded,
};
use wg_store::Session;
use wg_storecodec::Raw;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::edit::{Enrichment, enrichment_from};
use wg_trackers::{
    MappingConfig, PAGE_SIZE, SyncReport, TrackerSnapshot, cursor_name, dependency_order,
    fetch_candidates, listed_ids, map_issue, newest_update, referenced_keys, since_seconds,
    timestamp_ms,
};
use wg_types::pyval::{text, truthy};
use wg_types::{Correlation, Id, TaskKey, WgError};

use crate::spec;
use crate::spec::run::{TrackerSettings, settings_of, sync_json};
use crate::spec::sql;
use crate::spec::sync::{FinishLoad, PageCounts, PageInput, PageLoad, ReconcileCounts};

/// The `system` component a sync acts as (`SYNC_COMPONENT`).
const SYNC_COMPONENT: &str = "tracker-sync";

/// Where a sync reads issues from: the Jira client, or a fake in tests. The Jira client sends only
/// GET requests.
pub trait IssueSource {
    /// Issues in `projects` updated since `since` (epoch seconds), or every issue when `None`.
    fn search_updated(
        &mut self,
        projects: &[String],
        since: Option<i64>,
    ) -> Result<Vec<Json>, TrackerError>;
    /// `(external_id, key)` of every issue in `projects`.
    fn list_keys(&mut self, projects: &[String]) -> Result<Vec<(String, String)>, TrackerError>;
    /// One issue by key, or `None` when it does not exist or is not visible.
    fn get_issue(&mut self, key: &str) -> Result<Option<Json>, TrackerError>;
    /// The pull requests of the development panel of one issue.
    fn dev_pull_requests(&mut self, external_id: &str) -> Vec<Json>;
}

impl IssueSource for JiraClient {
    fn search_updated(
        &mut self,
        projects: &[String],
        since: Option<i64>,
    ) -> Result<Vec<Json>, TrackerError> {
        JiraClient::search_updated(self, projects, since)
    }

    fn list_keys(&mut self, projects: &[String]) -> Result<Vec<(String, String)>, TrackerError> {
        JiraClient::list_keys(self, projects)
    }

    fn get_issue(&mut self, key: &str) -> Result<Option<Json>, TrackerError> {
        JiraClient::get_issue(self, key)
    }

    fn dev_pull_requests(&mut self, external_id: &str) -> Vec<Json> {
        JiraClient::dev_pull_requests(self, external_id)
    }
}

/// The Jira client of a connection (`make_jira_client`): the credentials come from the variables
/// the connection names, and Cloud also needs the account email.
pub fn jira_client(settings: &TrackerSettings) -> Result<JiraClient, WgError> {
    let token = credential(&settings.token_env)?;
    let flavour = match settings.flavour.as_str() {
        "cloud" => {
            let Some(email_env) = settings.email_env.as_deref() else {
                return Err(WgError::config(
                    "Jira Cloud needs email_env: the variable holding the account email",
                ));
            };
            Flavour::Cloud {
                email: credential(email_env)?,
                token,
            }
        }
        _ => Flavour::DataCenter { token },
    };
    Ok(JiraClient {
        base_url: settings.base_url.clone(),
        flavour,
        timeout_seconds: settings.timeout_seconds,
        net: Net::default(),
        zone: None,
    })
}

/// `tracker sync CONNECTION [--full]` run now: the connection's settings, the Jira client, and one
/// sync.
pub fn run_now(
    app: &mut App,
    context: &RequestContext,
    connection: &str,
    full: bool,
) -> Result<SyncReport, WgError> {
    let config = app.config()?;
    let settings = settings_of(&config, connection)?;
    let enrich = enrichment_from(&config);
    sync(
        app,
        || jira_client(&settings),
        context.correlation.clone(),
        &settings,
        enrich,
        full,
    )
}

/// The `tracker_sync` job handler: the connection and mode of the payload, one sync. Its result is
/// the report as a JSON object.
pub fn tracker_sync_job(queue: &mut Queue, ctx: &mut JobContext) -> HandlerResult {
    let payload = ctx.job.payload.clone();
    let connection = text(payload.get("connection")).to_owned();
    let full = payload.get("full").is_some_and(truthy);
    let config = queue.app.config().map_err(HandlerError::Failed)?;
    let settings = settings_of(&config, &connection).map_err(HandlerError::Failed)?;
    let enrich = enrichment_from(&config);
    let correlation = match ctx.job.correlation_id.clone() {
        Some(correlation) => correlation,
        None => Correlation::new(
            wg_runtime::new_id()
                .map_err(HandlerError::Failed)?
                .to_string(),
        ),
    };
    let report = sync(
        &mut queue.app,
        || jira_client(&settings),
        correlation,
        &settings,
        enrich,
        full,
    )
    .map_err(HandlerError::Failed)?;
    Ok(Some(sync_json(&report)))
}

/// `TrackerSyncService.sync`: fetches the issues changed since the connection's cursor (every issue
/// when `full`), applies them page by page, reconciles a full sync and writes the cursors.
pub fn sync<S: IssueSource>(
    app: &mut App,
    make_source: impl FnOnce() -> Result<S, WgError>,
    correlation: Correlation,
    settings: &TrackerSettings,
    enrich: Enrichment,
    full: bool,
) -> Result<SyncReport, WgError> {
    let workspace = app.workspace_id(&settings.workspace)?;
    let sync_context = RequestContext::system(SYNC_COMPONENT, correlation)?;
    let since = if full {
        None
    } else {
        let cursor = read_cursor(app, &cursor_name(&settings.name, "updated"))?;
        since_seconds(cursor, settings.overlap_seconds)
    };
    let mut source = make_source()?;
    let issues = source
        .search_updated(&settings.projects, since)
        .map_err(spec::run::tracker_failure)?;
    let mut report = SyncReport::new(&settings.name, full);
    report.fetched = i64::try_from(issues.len()).unwrap_or(i64::MAX);
    for page in issues.chunks(PAGE_SIZE) {
        apply_page(
            app,
            &mut source,
            &sync_context,
            settings,
            workspace,
            enrich,
            page,
            &mut report,
        )?;
    }
    if full {
        reconcile_listing(
            app,
            &mut source,
            &sync_context,
            settings,
            workspace,
            &mut report,
        )?;
    }
    finish(app, &sync_context, settings, full, &mut report)?;
    Ok(report)
}

/// `_apply_page`: maps one page, fetches the parents and blockers it names, and applies the lot in
/// one transaction with the cursor.
#[allow(clippy::too_many_arguments)]
fn apply_page<S: IssueSource>(
    app: &mut App,
    source: &mut S,
    sync_context: &RequestContext,
    settings: &TrackerSettings,
    workspace: Id,
    enrich: Enrichment,
    issues: &[Json],
    report: &mut SyncReport,
) -> Result<(), WgError> {
    let searched = issues
        .iter()
        .map(|issue| map_one(source, settings, issue))
        .collect::<Result<Vec<_>, _>>()?;
    let fetched = fetch_referenced(app, source, settings, workspace, &searched, report)?;
    let batch: Vec<TrackerSnapshot> = searched.iter().cloned().chain(fetched).collect();
    let ordered = dependency_order(&batch);
    let name = cursor_name(&settings.name, "updated");
    let input_cursor = newest_update(&searched).map(timestamp_ms);
    let counts = app
        .runtime
        .decide_then(
            sync_context,
            |session| page_load(session, workspace, &name, &batch),
            |load: &PageLoad, stamp| {
                let input = PageInput {
                    connection: &settings.name,
                    workspace_id: workspace,
                    cancelled_resolutions: &settings.cancelled_resolutions,
                    enrich,
                    cursor_name: &name,
                    cursor_candidate: input_cursor,
                    snapshots: &ordered,
                };
                spec::sync::apply_page(load, &input, stamp)
            },
            index_written,
        )?
        .reply;
    merge_page(report, counts);
    Ok(())
}

/// Maps one issue with its development-panel pull requests (`_map`).
fn map_one<S: IssueSource>(
    source: &mut S,
    settings: &TrackerSettings,
    issue: &Json,
) -> Result<TrackerSnapshot, WgError> {
    let pull_requests = if settings.dev_panel {
        let external_id = text(issue.get("id"));
        source.dev_pull_requests(external_id)
    } else {
        Vec::new()
    };
    let config = MappingConfig {
        base_url: settings.base_url.clone(),
        pr_fields: settings.pr_fields.clone(),
        dev_panel: settings.dev_panel,
    };
    map_issue(issue, &pull_requests, &config).map_err(|err| WgError::workgraph(err.message()))
}

/// `_fetch_referenced`: the parents and blockers of the searched issues that are not synced yet and
/// live in a configured project, fetched breadth first. Fetch failures are reported, not raised.
fn fetch_referenced<S: IssueSource>(
    app: &mut App,
    source: &mut S,
    settings: &TrackerSettings,
    workspace: Id,
    searched: &[TrackerSnapshot],
    report: &mut SyncReport,
) -> Result<Vec<TrackerSnapshot>, WgError> {
    let mut have: HashSet<String> = searched.iter().map(|snap| snap.key.clone()).collect();
    let mut pending: Vec<String> = searched
        .iter()
        .flat_map(referenced_keys)
        .map(str::to_owned)
        .collect();
    let mut fetched = Vec::new();
    while !pending.is_empty() {
        let referenced: Vec<&str> = pending.iter().map(String::as_str).collect();
        let wanted = fetch_candidates(&referenced, &have, &settings.projects);
        pending = Vec::new();
        if wanted.is_empty() {
            break;
        }
        let known = known_keys(app, workspace, &wanted)?;
        for key in wanted {
            if known.contains(&key) {
                continue;
            }
            have.insert(key.clone());
            match source.get_issue(&key) {
                Err(err) => report.errors.push(format!("fetch {key}: {}", err.message)),
                Ok(None) => {}
                Ok(Some(issue)) => {
                    let snap = map_one(source, settings, &issue)?;
                    pending.extend(referenced_keys(&snap).into_iter().map(str::to_owned));
                    fetched.push(snap);
                }
            }
        }
    }
    Ok(fetched)
}

/// The keys among `keys` that a task of the workspace already has.
fn known_keys(app: &mut App, workspace: Id, keys: &[String]) -> Result<HashSet<String>, WgError> {
    app.runtime.query(
        |session| {
            let rows = session.rows(&sql::known_task_keys(workspace, keys))?;
            rows.iter()
                .map(|values| match values.first() {
                    Some(Raw::Text(key)) => Ok(key.clone()),
                    _ => Err(wg_storecodec::db_error(
                        wg_storecodec::DbFailure::Other,
                        "a task key row is not text",
                    )),
                })
                .collect()
        },
        |known: &HashSet<String>| Ok(known.clone()),
    )
}

/// The rows a page reads: the task world, the `updated` cursor, and the queued jobs of the tasks
/// the page names (the enrichment enqueue is idempotent against them).
fn page_load(
    session: &mut Session<'_>,
    workspace: Id,
    cursor_name: &str,
    batch: &[TrackerSnapshot],
) -> Result<PageLoad, WgError> {
    let world: TaskWorld = session.task_world(Some(workspace), &[])?;
    let cursor = session.system_cursor(cursor_name)?;
    let mut queue = QueueSlice::default();
    let mut succeeded = Vec::new();
    for snap in batch {
        let Ok(key) = TaskKey::parse(&snap.key) else {
            continue;
        };
        let Some(task) = world.tasks.task_by_key(workspace, &key) else {
            continue;
        };
        succeeded.extend(latest_succeeded(session, task.id)?);
        let jobs = session.task_jobs_slice(task.id)?;
        for (id, job) in jobs.jobs.iter() {
            queue.jobs = queue.jobs.insert(*id, job.clone());
        }
    }
    Ok(PageLoad {
        world,
        succeeded,
        queue,
        cursor,
    })
}

/// Adds a page's counts to the report.
fn merge_page(report: &mut SyncReport, counts: PageCounts) {
    report.created += counts.created;
    report.updated += counts.updated;
    report.unchanged += counts.unchanged;
    report.errors.extend(counts.errors);
    for url in counts.unresolved {
        if !report.unresolved_pull_requests.contains(&url) {
            report.unresolved_pull_requests.push(url);
        }
    }
}

/// `_reconcile`: lists the connection's issues and reconciles the tracked items against them.
fn reconcile_listing<S: IssueSource>(
    app: &mut App,
    source: &mut S,
    sync_context: &RequestContext,
    settings: &TrackerSettings,
    workspace: Id,
    report: &mut SyncReport,
) -> Result<(), WgError> {
    let keys = source
        .list_keys(&settings.projects)
        .map_err(spec::run::tracker_failure)?;
    let listed: HashSet<String> = listed_ids(&keys);
    let counts: ReconcileCounts = app
        .runtime
        .decide_then(
            sync_context,
            |session| session.task_world(Some(workspace), &[]),
            |world: &TaskWorld, stamp| {
                Ok(spec::sync::reconcile_page(
                    world,
                    &settings.name,
                    &listed,
                    settings.missing_grace_seconds,
                    stamp,
                    workspace,
                ))
            },
            index_written,
        )?
        .reply;
    report.missing = counts.missing;
    report.archived += counts.archived;
    Ok(())
}

/// `_finish`: `last_sync` (and `last_full_sync` for a full sync) move to now, and the report takes
/// the `updated` cursor.
fn finish(
    app: &mut App,
    sync_context: &RequestContext,
    settings: &TrackerSettings,
    full: bool,
    report: &mut SyncReport,
) -> Result<(), WgError> {
    let updated = cursor_name(&settings.name, "updated");
    let last_sync = cursor_name(&settings.name, "last_sync");
    let last_full = cursor_name(&settings.name, "last_full_sync");
    report.cursor = app
        .runtime
        .decide(
            sync_context,
            |session| {
                Ok(FinishLoad {
                    updated: session.system_cursor(&updated)?,
                    last_sync: session.system_cursor(&last_sync)?,
                    last_full_sync: session.system_cursor(&last_full)?,
                })
            },
            |load: &FinishLoad, stamp| {
                Ok(spec::sync::finish_page(load, &settings.name, full, stamp))
            },
        )?
        .reply;
    Ok(())
}

/// The value of a cursor row, in epoch milliseconds.
fn read_cursor(app: &mut App, name: &str) -> Result<Option<i64>, WgError> {
    app.runtime.query(
        |session| Ok(session.system_cursor(name)?.map(|row| row.value)),
        |value: &Option<i64>| Ok(*value),
    )
}

/// The credential a variable holds (`_credential`): an unset or empty variable is a config error.
fn credential(name: &str) -> Result<String, WgError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            WgError::config(format!(
                "environment variable {name} is not set; Jira credentials are read from it"
            ))
        })
}
