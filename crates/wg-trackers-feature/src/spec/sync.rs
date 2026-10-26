//! Pure: the writes of one tracker sync (`TrackerSyncService._apply_page`, `_reconcile` and
//! `_finish` in `integrations/trackers/sync.py`). Each function takes the rows a page loaded and
//! the snapshots the shell fetched, and returns the decision the shell applies in one transaction.
//!
//! The sync's own rules (what a snapshot changes, which references resolve, what a reconcile
//! archives) are the `wg-trackers` plans; the task rules (create, update, status, dependencies)
//! are the `wg-tasks` plans. This module threads them: each sub-operation draws its ids from its
//! own sub-stamp, and the rows it writes are applied to the state before the next one reads it.

use std::collections::{BTreeSet, HashSet};

use serde_json::Value;
use wg_model::entity::{
    ExternalItem, JsonObject, Project, SystemCursor, Task, TaskDependency, TaskLink,
    TaskRepository, Workspace,
};
use wg_model::event::{
    Changes, EventDraft, EventPayload, ProjectCreated, TaskDependencyChanged, TaskKeyed,
    TaskRepositoryLinked, TaskSynced, TaskUpdated,
};
use wg_model::slice::{QueueSlice, StackSlice};
use wg_model::stamp::{IdSeq, Stamp};
use wg_model::{Decision, RowKey, TaskSlice, Write};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::accept::substamp;
use wg_tasks::create::{Allocation, NewTask, RepositoryLink, create_plan};
use wg_tasks::deps::depends_on_reachable;
use wg_tasks::edit::{Enrichment, UpdateRead, enrichment_jobs, update_task};
use wg_tasks::hierarchy::is_ancestor;
use wg_tasks::transition::{Applied, Change, refresh_one, transition};
use wg_trackers::{
    BlockerTask, ExistingDependency, FieldUpdates, LinkChange, LinkValues, ParentFields,
    PullRequestRef, ReconcileAction, ReconcileItem, StoredLink, SyncLinks, TaskFields,
    TrackerSnapshot, changed_fields, cursor_name, dependency_plan, field_updates, project_slug,
    pull_request_changes, reconcile, repository_list, snapshot_hash, timestamp_ms,
    tracker_target_status,
};
use wg_types::{ErrorCode, Id, TaskKey, TaskStatus, Timestamp, WgError, parse_id};

/// The `created_by` of the rows a sync writes.
const SYNC_ACTOR: &str = wg_trackers::SYNC_ACTOR;
/// The `kind` of a pull request link.
const PULL_REQUEST_KIND: &str = wg_trackers::PULL_REQUEST_KIND;

/// What one page changed, as the report counts it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageCounts {
    pub created: i64,
    pub updated: i64,
    pub unchanged: i64,
    pub errors: Vec<String>,
    /// Pull request repositories no registered repository matches, first-seen, without repeats.
    pub unresolved: Vec<String>,
}

/// What a full sync's reconcile changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileCounts {
    pub missing: i64,
    pub archived: i64,
}

/// What the shell gives one page to apply.
pub struct PageInput<'a> {
    pub connection: &'a str,
    pub workspace_id: Id,
    /// `cancelled_resolutions` of the connection.
    pub cancelled_resolutions: &'a [String],
    /// `llm.auto_classify` and `llm.auto_embed`.
    pub enrich: Enrichment,
    /// The name of the `updated` cursor row.
    pub cursor_name: &'a str,
    /// The newest remote update the page searched, in epoch milliseconds.
    pub cursor_candidate: Option<i64>,
    /// The snapshots to apply, parents and blockers first.
    pub snapshots: &'a [&'a TrackerSnapshot],
}

/// The rows a page reads: the task world, and the jobs that name the tasks it touches (the
/// enrichment enqueue is idempotent against them).
pub struct PageLoad {
    pub world: TaskWorld,
    pub queue: QueueSlice,
    /// The `updated` cursor row, if it exists.
    pub cursor: Option<SystemCursor>,
}

/// The rows the finish of a sync reads: the `updated` cursor and the two sync-time cursors.
pub struct FinishLoad {
    pub updated: Option<SystemCursor>,
    pub last_sync: Option<SystemCursor>,
    pub last_full_sync: Option<SystemCursor>,
}

/// `_finish`: `last_sync` (and `last_full_sync` for a full sync) move to now. The reply is the
/// `updated` cursor after the sync, in epoch milliseconds.
#[must_use]
pub fn finish_page(
    load: &FinishLoad,
    connection: &str,
    full: bool,
    stamp: &Stamp,
) -> Decision<Option<i64>> {
    let now = timestamp_ms(stamp.at);
    let last_sync = cursor_write(
        load.last_sync.as_ref(),
        &cursor_name(connection, "last_sync"),
        now,
        stamp.at,
    );
    let last_full = full
        .then(|| {
            cursor_write(
                load.last_full_sync.as_ref(),
                &cursor_name(connection, "last_full_sync"),
                now,
                stamp.at,
            )
        })
        .flatten();
    let reply = load.updated.as_ref().map(|row| row.value);
    [last_sync, last_full]
        .into_iter()
        .flatten()
        .fold(Decision::new(reply), Decision::write)
}

/// Applies one page of snapshots (`_apply_page`). The decision holds the writes, the events and
/// the counts; the cursor only moves forward.
pub fn apply_page(
    load: &PageLoad,
    input: &PageInput,
    stamp: &Stamp,
) -> Result<Decision<PageCounts>, WgError> {
    let mut page = Page::new(load, input, stamp);
    for snap in input.snapshots {
        page.apply_one(snap)?;
    }
    if let Some(candidate) = input.cursor_candidate {
        page.cursor(load.cursor.as_ref(), input.cursor_name, candidate);
    }
    let counts = page.counts.clone();
    Ok(page.into_decision(counts))
}

/// Reconciles a full listing (`_reconcile`): restores what the listing returned, marks what it did
/// not, and archives the tasks whose mark is older than the grace period.
pub fn reconcile_page(
    world: &TaskWorld,
    connection: &str,
    listed: &HashSet<String>,
    grace_seconds: i64,
    stamp: &Stamp,
    workspace_id: Id,
) -> Decision<ReconcileCounts> {
    let mut items: Vec<&ExternalItem> = world
        .tasks
        .external_items
        .values()
        .filter(|item| item.connection == connection)
        .collect();
    items.sort_by_key(|item| (item.created_at, item.id));
    let views: Vec<ReconcileItem> = items
        .iter()
        .map(|item| ReconcileItem {
            external_id: item.external_id.as_str(),
            task_archived: world
                .tasks
                .task(item.task_id)
                .map(|task| task.archived_at.is_some()),
            missing_since: item.missing_since,
        })
        .collect();
    let actions = reconcile(&views, listed, stamp.at, grace_seconds);
    let mut page = Page::over(
        world,
        stamp,
        workspace_id,
        connection,
        &[],
        Enrichment::default(),
        QueueSlice::default(),
    );
    for (item, action) in items.into_iter().zip(actions) {
        page.reconcile_one(item, action);
    }
    let counts = ReconcileCounts {
        missing: page.missing,
        archived: page.archived,
    };
    page.into_decision(counts)
}

/// The cursor row a write moves: `_write_cursor`, which only ever moves a cursor forward.
#[must_use]
pub fn cursor_write(
    existing: Option<&SystemCursor>,
    name: &str,
    candidate: i64,
    at: Timestamp,
) -> Option<Write> {
    match existing {
        None => Some(Write::insert(SystemCursor {
            name: name.to_owned(),
            value: candidate,
            updated_at: at,
        })),
        Some(row) => {
            let value = row.value.max(candidate);
            (value != row.value).then(|| {
                Write::update(SystemCursor {
                    value,
                    updated_at: at,
                    ..row.clone()
                })
            })
        }
    }
}

/// The running state of one transaction: the task rows as the writes so far leave them, the
/// writes and events in order, and the counts.
struct Page {
    stamp: Stamp,
    workspace_id: Id,
    connection: String,
    cancelled: Vec<String>,
    enrich: Enrichment,
    queue: QueueSlice,
    stacks: StackSlice,
    tasks: TaskSlice,
    writes: Vec<Write>,
    events: Vec<EventDraft>,
    salt: u32,
    counts: PageCounts,
    missing: i64,
    archived: i64,
}

impl Page {
    fn new(load: &PageLoad, input: &PageInput, stamp: &Stamp) -> Self {
        Page::over(
            &load.world,
            stamp,
            input.workspace_id,
            input.connection,
            input.cancelled_resolutions,
            input.enrich,
            load.queue.clone(),
        )
    }

    fn over(
        world: &TaskWorld,
        stamp: &Stamp,
        workspace_id: Id,
        connection: &str,
        cancelled: &[String],
        enrich: Enrichment,
        queue: QueueSlice,
    ) -> Self {
        Page {
            stamp: stamp.clone(),
            workspace_id,
            connection: connection.to_owned(),
            cancelled: cancelled.to_vec(),
            enrich,
            queue,
            stacks: world.stacks.clone(),
            tasks: world.tasks.clone(),
            writes: Vec::new(),
            events: Vec::new(),
            salt: 0,
            counts: PageCounts::default(),
            missing: 0,
            archived: 0,
        }
    }

    fn into_decision<R>(self, reply: R) -> Decision<R> {
        let decision = self
            .writes
            .into_iter()
            .fold(Decision::new(reply), Decision::write);
        self.events.into_iter().fold(decision, Decision::event)
    }

    /// A sub-stamp of the transaction's stamp, distinct from every earlier one of it.
    fn next_stamp(&mut self) -> Stamp {
        self.salt += 1;
        substamp(&self.stamp, self.salt)
    }

    /// Records one write and applies it to the state.
    fn write(&mut self, write: Write) {
        self.tasks = self.tasks.apply(&write);
        self.writes.push(write);
    }

    /// Appends a sub-decision's writes and events, applying its writes to the state.
    fn absorb(&mut self, decision: &Decision<()>) {
        for write in decision.writes.iter() {
            self.write(write.clone());
        }
        for event in decision.events.iter() {
            self.events.push(event.clone());
        }
    }

    /// Records one event under a fresh sub-stamp and returns its id.
    fn event(&mut self, payload: EventPayload, entity_id: Id, caused_by: Option<Id>) -> Id {
        let sub = self.next_stamp();
        let (id, _) = sub.ids().next();
        let draft = EventDraft::new(&sub, id, Some(self.workspace_id), entity_id, payload);
        let draft = match caused_by {
            Some(cause) => draft.caused_by(cause),
            None => draft,
        };
        self.events.push(draft);
        id
    }

    fn task(&self, id: Id) -> Result<Task, WgError> {
        self.tasks
            .task(id)
            .cloned()
            .ok_or_else(|| WgError::new(ErrorCode::WorkgraphError, format!("task {id} is missing")))
    }

    /// The workspace's task with `key`, as the state holds it now.
    fn task_by_key(&self, key: &str) -> Option<Task> {
        TaskKey::parse(key)
            .ok()
            .and_then(|parsed| self.tasks.task_by_key(self.workspace_id, &parsed).cloned())
    }

    /// `task.version += 1`, written. Returns the task as now stored.
    fn bump(&mut self, task_id: Id) -> Result<Task, WgError> {
        let task = self.task(task_id)?;
        let next = Task {
            version: task.version + 1,
            ..task
        };
        self.write(Write::update(next.clone()));
        Ok(next)
    }

    /// `_ensure_project`: the project for a Jira project key, created when it does not exist.
    fn ensure_project(&mut self, project_key: &str) {
        let slug = project_slug(project_key);
        if self.project_id(&slug).is_some() {
            return;
        }
        let sub = self.next_stamp();
        let ([project_id, event_id], _) = sub.ids().take::<2>();
        let project = Project {
            id: project_id,
            workspace_id: self.workspace_id,
            slug: slug.clone(),
            name: project_key.to_owned(),
            description: String::new(),
            status: "active".to_owned(),
            settings: JsonObject::new(),
            created_at: self.stamp.at,
            updated_at: self.stamp.at,
        };
        self.write(Write::insert(project));
        let payload = EventPayload::ProjectCreated(ProjectCreated {
            slug,
            name: project_key.to_owned(),
            repository_ids: Default::default(),
        });
        self.events.push(EventDraft::new(
            &sub,
            event_id,
            Some(self.workspace_id),
            project_id,
            payload,
        ));
    }

    /// The id of the workspace project with `slug`, if it exists.
    fn project_id(&self, slug: &str) -> Option<Id> {
        self.tasks
            .registry
            .projects
            .values()
            .find(|project| project.workspace_id == self.workspace_id && project.slug == slug)
            .map(|project| project.id)
    }

    /// `resolve_repository`: the non-archived repository of the workspace whose remote matches the
    /// URL. More than one match resolves to nothing.
    fn resolve_repository(&self, url: &str) -> Option<Id> {
        let wanted = url.to_lowercase();
        let matches: BTreeSet<Id> = self
            .tasks
            .registry
            .remotes
            .values()
            .filter(|remote| remote.normalized_url.to_lowercase() == wanted)
            .filter_map(|remote| self.tasks.registry.repositories.get(&remote.repository_id))
            .filter(|repo| repo.workspace_id == self.workspace_id && repo.archived_at.is_none())
            .map(|repo| repo.id)
            .collect();
        match matches.len() {
            1 => matches.into_iter().next(),
            _ => None,
        }
    }

    /// `_resolve_pull_request_repos`: the registered repository per distinct repository URL of the
    /// snapshot, in first-seen order. Unmatched URLs are also reported, once for the whole page.
    fn resolve_repositories(&mut self, snap: &TrackerSnapshot) -> Vec<(String, Option<Id>)> {
        let urls: Vec<&str> = snap
            .pull_requests
            .iter()
            .map(|pr| pr.repo_normalized_url.as_str())
            .collect();
        let distinct: Vec<&str> = urls
            .iter()
            .copied()
            .enumerate()
            .filter(|(index, url)| !urls[..*index].contains(url))
            .map(|(_, url)| url)
            .collect();
        let resolved: Vec<(String, Option<Id>)> = distinct
            .into_iter()
            .map(|url| (url.to_owned(), self.resolve_repository(url)))
            .collect();
        for (url, repo) in &resolved {
            if repo.is_none() && !self.counts.unresolved.contains(url) {
                self.counts.unresolved.push(url.clone());
            }
        }
        resolved
    }

    /// The blockers of a snapshot that exist in the workspace: each key once, not the issue itself.
    fn blockers_of(&self, snap: &TrackerSnapshot) -> Vec<Task> {
        let keys: Vec<&str> = snap.blocked_by_keys.iter().map(String::as_str).collect();
        keys.iter()
            .copied()
            .enumerate()
            .filter(|(index, key)| !keys[..*index].contains(key))
            .filter_map(|(_, key)| self.task_by_key(key))
            .filter(|task| task.key.to_string() != snap.key)
            .collect()
    }

    /// The tracked item of this issue on this connection, as the state holds it.
    fn item_of(&self, external_id: &str) -> Option<ExternalItem> {
        self.tasks
            .external_items
            .values()
            .find(|item| item.connection == self.connection && item.external_id == external_id)
            .cloned()
    }

    /// Applies one snapshot (`_apply`).
    fn apply_one(&mut self, snap: &TrackerSnapshot) -> Result<(), WgError> {
        let resolved = self.resolve_repositories(snap);
        let repo_texts: Vec<Option<String>> = resolved
            .iter()
            .map(|(_, repo)| repo.map(|id| id.to_string()))
            .collect();
        let repos = repository_list(&repo_texts);
        let parent = snap
            .parent_key
            .as_deref()
            .and_then(|key| self.task_by_key(key));
        let blockers = self.blockers_of(snap);
        let links = SyncLinks {
            repositories: repos.clone(),
            parent: parent.as_ref().map(|task| task.id.to_string()),
            blockers: sorted(blockers.iter().map(|task| task.id.to_string()).collect()),
        };
        let digest = snapshot_hash(snap, &links);
        let now = self.stamp.at;
        let item = self.item_of(&snap.external_id);

        if let Some(item) = item.as_ref().filter(|item| item.snapshot_hash == digest) {
            let mut row = ExternalItem {
                synced_at: Some(now),
                ..item.clone()
            };
            let task = self.task(item.task_id)?;
            self.restore(&mut row, &task);
            self.write(Write::update(row));
            self.counts.unchanged += 1;
            return Ok(());
        }

        self.ensure_project(&snap.project_key);
        let project_id = self.project_id(&project_slug(&snap.project_key));
        let mut changed: Vec<&'static str> = Vec::new();
        let created = item.is_none();
        let mut row = match item {
            Some(existing) => {
                let mut row = existing.clone();
                let task = self.task(existing.task_id)?;
                self.restore(&mut row, &task);
                if snap.key != task.key.to_string() {
                    if self.task_by_key(&snap.key).is_some() {
                        self.counts.errors.push(format!(
                            "{}: Jira now calls it {}, which a local task already uses",
                            task.key, snap.key
                        ));
                        self.write(Write::update(row));
                        return Ok(());
                    }
                    self.rename(&task, &snap.key)?;
                    changed.push("key");
                }
                let current = self.task(existing.task_id)?;
                let parent_fields = parent
                    .as_ref()
                    .map(|parent| (parent.id.to_string(), parent.key.to_string()));
                let updates = self.field_updates_of(&current, snap, parent.as_ref(), parent_fields);
                self.counts.errors.extend(updates.errors.iter().cloned());
                if updates.has_changes() {
                    changed.extend(update_names(&updates));
                    self.update_task_fields(&current, &updates)?;
                }
                row
            }
            None => {
                if self.task_by_key(&snap.key).is_some() {
                    self.counts.errors.push(format!(
                        "{}: a local task already uses this key; not mirrored",
                        snap.key
                    ));
                    return Ok(());
                }
                let task_id = self.create_task(snap, parent.as_ref(), &repos, project_id)?;
                let sub = self.next_stamp();
                let (item_id, _) = sub.ids().next();
                blank_item(
                    item_id,
                    self.workspace_id,
                    task_id,
                    &self.connection,
                    &snap.external_id,
                    now,
                )
            }
        };
        store_item(&mut row, &self.connection, snap, &digest, now);
        self.write(if created {
            Write::insert(row.clone())
        } else {
            Write::update(row.clone())
        });

        let task = self.task(row.task_id)?;
        if self.sync_pull_requests(&task, snap, &resolved) {
            changed.push("pull_requests");
        }
        if self.sync_repositories(&task, &repos)? {
            changed.push("repositories");
        }
        if self.sync_dependencies(&task, &blockers)? {
            changed.push("dependencies");
        }
        self.apply_tracker_status(&task, &row, &mut changed)?;

        if created {
            self.counts.created += 1;
        } else if changed.is_empty() {
            self.counts.unchanged += 1;
        } else {
            self.counts.updated += 1;
        }
        if created || !changed.is_empty() {
            let fields = changed_fields(&changed).into_iter().collect();
            self.event(
                EventPayload::TaskSynced(TaskSynced {
                    connection: self.connection.clone(),
                    external_key: snap.key.clone(),
                    created,
                    changed_fields: fields,
                }),
                task.id,
                None,
            );
        }
        Ok(())
    }

    /// `_restore`: an issue that reappeared clears its mark, and a task only the grace period
    /// archived is unarchived.
    fn restore(&mut self, row: &mut ExternalItem, task: &Task) {
        if row.missing_since.is_none() {
            return;
        }
        row.missing_since = None;
        if task.archived_at.is_some() {
            self.unarchive(task);
        }
    }

    fn unarchive(&mut self, task: &Task) {
        let next = Task {
            archived_at: None,
            version: task.version + 1,
            ..task.clone()
        };
        self.write(Write::update(next));
        self.event(
            EventPayload::TaskUnarchived(TaskKeyed {
                key: task.key.clone(),
            }),
            task.id,
            None,
        );
    }

    /// `_rename`: the tracker's new key for a tracked task.
    fn rename(&mut self, task: &Task, new_key: &str) -> Result<(), WgError> {
        let key = TaskKey::parse(new_key)?;
        let version = task.version + 1;
        let renamed = Task {
            key: key.clone(),
            version,
            ..task.clone()
        };
        self.write(Write::update(renamed));
        let changes = Changes::new().with(
            "key",
            Value::String(task.key.to_string()),
            Value::String(new_key.to_owned()),
        );
        self.event(
            EventPayload::TaskUpdated(TaskUpdated {
                key,
                changes,
                material_change: false,
                reason: Some("tracker key change".to_owned()),
                version,
            }),
            task.id,
            None,
        );
        Ok(())
    }

    /// `_field_updates` over the task as it now stands.
    fn field_updates_of(
        &self,
        task: &Task,
        snap: &TrackerSnapshot,
        parent: Option<&Task>,
        parent_fields: Option<(String, String)>,
    ) -> FieldUpdates {
        let task_key = task.key.to_string();
        let task_id = task.id.to_string();
        let parent_id = task.parent_id.map(|id| id.to_string());
        let fields = TaskFields {
            id: &task_id,
            key: &task_key,
            title: &task.title,
            description: &task.description,
            priority: task.priority,
            assignee: task.assignee.as_deref(),
            parent_id: parent_id.as_deref(),
        };
        let parent_view = parent_fields
            .as_ref()
            .map(|(id, key)| ParentFields { id, key });
        field_updates(&fields, snap, parent_view, || {
            parent.is_some_and(|parent| is_ancestor(&self.tasks, task.id, parent.id))
        })
    }

    /// `_update_in_session` with the sync's field updates.
    fn update_task_fields(&mut self, task: &Task, updates: &FieldUpdates) -> Result<(), WgError> {
        let read = UpdateRead {
            title: updates.title.clone(),
            description: updates.description.clone(),
            priority: updates.priority,
            assignee: updates.assignee.clone(),
            parent: updates.parent_key.clone(),
            clear_parent: updates.clear_parent,
            ..UpdateRead::default()
        };
        let sub = self.next_stamp();
        let world = TaskWorld {
            tasks: self.tasks.clone(),
            stacks: self.stacks.clone(),
        };
        let decision = update_task(
            &world,
            &self.queue,
            &sub,
            Some(self.workspace_id),
            &task.key.to_string(),
            read,
            self.enrich,
        )?;
        self.absorb(&decision.map(|_| ()));
        Ok(())
    }

    /// `_create_in_session` for an issue the workspace does not track yet. Returns the new task id.
    fn create_task(
        &mut self,
        snap: &TrackerSnapshot,
        parent: Option<&Task>,
        repos: &[String],
        project_id: Option<Id>,
    ) -> Result<Id, WgError> {
        let space: Workspace = self
            .tasks
            .registry
            .workspace(self.workspace_id)
            .cloned()
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, "workspace not found"))?;
        let repositories: Vec<RepositoryLink> = repos
            .iter()
            .filter_map(|text| self.repository_by_text(text))
            .map(|repo| RepositoryLink {
                repository_id: repo.id,
                access: repo.access_mode.as_str().to_owned(),
            })
            .collect();
        let sub = self.next_stamp();
        let plan = create_plan(
            &sub,
            Allocation {
                workspace_id: self.workspace_id,
                key_prefix: &space.key_prefix,
                number: space.next_task_number,
                key_taken: false,
            },
            NewTask {
                key: Some(snap.key.clone()),
                title: snap.title.clone(),
                description: snap.description.clone(),
                acceptance_criteria: Vec::new(),
                status: TaskStatus::Backlog,
                priority: snap.priority,
                parent_id: parent.map(|task| task.id),
                assignee: snap.assignee.clone(),
                assigned_agent: None,
                auto_execute: false,
                due_at: None,
                scheduled_at: None,
                estimated_effort: None,
                metadata: JsonObject::new(),
                tags: Vec::new(),
                files: Vec::new(),
                projects: project_id.into_iter().collect(),
                repositories,
                depends_on: Vec::new(),
            },
        )?;
        let task_id = plan.task.id;
        let dependency_count = plan.dependencies.len();
        self.write(Write::update(Workspace {
            next_task_number: space.next_task_number + 1,
            updated_at: self.stamp.at,
            ..space
        }));
        self.write(Write::insert(plan.task.clone()));
        for row in plan.tags {
            self.write(Write::insert(row));
        }
        for row in plan.files {
            self.write(Write::insert(row));
        }
        for row in plan.projects {
            self.write(Write::insert(row));
        }
        for row in plan.repositories {
            self.write(Write::insert(row));
        }
        for row in plan.dependencies {
            self.write(Write::insert(row));
        }
        // The plan drew the task id and one id per dependency; the event continues after them.
        let ids = (0..=dependency_count).fold(sub.ids(), |ids: IdSeq, _| ids.next().1);
        let (created_id, _) = ids.next();
        self.events.push(EventDraft::new(
            &sub,
            created_id,
            Some(self.workspace_id),
            task_id,
            EventPayload::TaskCreated(plan.event),
        ));
        let jobs = enrichment_jobs(&sub, &plan.task, created_id, self.enrich, &self.queue);
        self.absorb(&jobs);
        Ok(task_id)
    }

    fn repository_by_text(&self, text: &str) -> Option<wg_model::entity::Repository> {
        self.tasks
            .registry
            .repositories
            .values()
            .find(|repo| repo.id.to_string() == text)
            .cloned()
    }

    /// `_sync_pull_requests`: the task's pull request links, made to match the snapshot.
    fn sync_pull_requests(
        &mut self,
        task: &Task,
        snap: &TrackerSnapshot,
        resolved: &[(String, Option<Id>)],
    ) -> bool {
        let rows: Vec<TaskLink> = self
            .tasks
            .links_of(task.id)
            .filter(|link| link.kind == PULL_REQUEST_KIND)
            .cloned()
            .collect();
        let stored: Vec<StoredLink> = rows.iter().map(stored_link).collect();
        let wanted: Vec<(PullRequestRef, Option<String>)> = snap
            .pull_requests
            .iter()
            .map(|pr| {
                let repo = resolved
                    .iter()
                    .find(|(url, _)| *url == pr.repo_normalized_url)
                    .and_then(|(_, repo)| repo.map(|id| id.to_string()));
                (pr.clone(), repo)
            })
            .collect();
        let changes = pull_request_changes(&stored, &wanted);
        let changed = !changes.is_empty();
        for change in changes {
            match change {
                LinkChange::Insert {
                    normalized_url,
                    values,
                } => {
                    let sub = self.next_stamp();
                    let (id, _) = sub.ids().next();
                    let link = new_link(id, task.id, normalized_url, &values, self.stamp.at);
                    self.write(Write::insert(link));
                }
                LinkChange::Update { index, values } => {
                    let existing = &rows[index];
                    let link = TaskLink {
                        url: values.url.clone(),
                        provider: values.provider.clone(),
                        repo_full_name: values.repo_full_name.clone(),
                        number: values.number,
                        repository_id: values
                            .repository_id
                            .as_deref()
                            .and_then(|text| parse_id(text).ok()),
                        sources: values.sources.iter().cloned().collect(),
                        title: values.title.clone(),
                        ..existing.clone()
                    };
                    self.write(Write::update(link));
                }
                LinkChange::Delete { index } => {
                    self.write(Write::delete(RowKey::TaskLink(rows[index].id)));
                }
            }
        }
        changed
    }

    /// `_sync_repositories`: links each registered pull request repository to the task.
    fn sync_repositories(&mut self, task: &Task, repos: &[String]) -> Result<bool, WgError> {
        let mut changed = false;
        for text in repos {
            let Some(repo) = self.repository_by_text(text) else {
                continue;
            };
            if self
                .tasks
                .task_repositories
                .get(&(task.id, repo.id))
                .is_some()
            {
                continue;
            }
            let access = repo.access_mode.as_str().to_owned();
            self.write(Write::insert(TaskRepository {
                task_id: task.id,
                repository_id: repo.id,
                access: access.clone(),
                created_at: self.stamp.at,
            }));
            self.bump(task.id)?;
            self.event(
                EventPayload::TaskRepositoryLinked(TaskRepositoryLinked {
                    repository_id: repo.id,
                    repository: repo.name.clone(),
                    access,
                }),
                task.id,
                None,
            );
            changed = true;
        }
        Ok(changed)
    }

    /// `_sync_dependencies`: the sync-created dependencies of the task follow its blockers. The
    /// dependencies people created are left alone.
    fn sync_dependencies(&mut self, task: &Task, blockers: &[Task]) -> Result<bool, WgError> {
        let rows: Vec<TaskDependency> = self.tasks.dependencies_of(task.id).cloned().collect();
        let row_ids: Vec<String> = rows
            .iter()
            .map(|row| row.depends_on_id.to_string())
            .collect();
        let existing: Vec<ExistingDependency> = rows
            .iter()
            .zip(row_ids.iter())
            .map(|(row, id)| ExistingDependency {
                depends_on_id: id.as_str(),
                sync_created: row.created_by == SYNC_ACTOR,
            })
            .collect();
        let blocker_ids: Vec<String> = blockers.iter().map(|b| b.id.to_string()).collect();
        let blocker_keys: Vec<String> = blockers.iter().map(|b| b.key.to_string()).collect();
        let views: Vec<BlockerTask> = blocker_ids
            .iter()
            .zip(blocker_keys.iter())
            .map(|(id, key)| BlockerTask {
                id: id.as_str(),
                key: key.as_str(),
            })
            .collect();
        let cycles: HashSet<String> = blockers
            .iter()
            .filter(|blocker| !row_ids.contains(&blocker.id.to_string()))
            .filter(|blocker| depends_on_reachable(&self.tasks, blocker.id, task.id).is_some())
            .map(|blocker| blocker.id.to_string())
            .collect();
        let task_key = task.key.to_string();
        let plan = dependency_plan(&task_key, &existing, &views, |id| cycles.contains(id));
        self.counts.errors.extend(plan.errors.iter().cloned());
        let changed = !plan.add.is_empty() || !plan.remove.is_empty();
        for blocker_id in &plan.add {
            let Some(blocker) = blockers.iter().find(|b| b.id.to_string() == *blocker_id) else {
                continue;
            };
            let sub = self.next_stamp();
            let (row_id, _) = sub.ids().next();
            self.write(Write::insert(TaskDependency {
                id: row_id,
                task_id: task.id,
                depends_on_id: blocker.id,
                created_by: SYNC_ACTOR.to_owned(),
                created_at: self.stamp.at,
            }));
            let payload = EventPayload::TaskDependencyAdded(TaskDependencyChanged {
                depends_on: blocker.id,
                depends_on_key: blocker.key.clone(),
            });
            self.dependency_changed(task.id, payload)?;
        }
        for blocker_id in &plan.remove {
            let Some(row) = rows
                .iter()
                .find(|row| row.depends_on_id.to_string() == *blocker_id)
            else {
                continue;
            };
            let key = self.task(row.depends_on_id)?.key;
            self.write(Write::delete(RowKey::TaskDependency(row.id)));
            let payload = EventPayload::TaskDependencyRemoved(TaskDependencyChanged {
                depends_on: row.depends_on_id,
                depends_on_key: key,
            });
            self.dependency_changed(task.id, payload)?;
        }
        Ok(changed)
    }

    /// `_dependency_changed`: the task's version moves, the event is recorded, and the task is
    /// refreshed against its blockers (`_refresh_blocked_state`, caused by the event).
    fn dependency_changed(&mut self, task_id: Id, payload: EventPayload) -> Result<(), WgError> {
        self.bump(task_id)?;
        let event_id = self.event(payload, task_id, None);
        let sub = self.next_stamp();
        let start = Applied {
            decision: Decision::new(()),
            ids: sub.ids(),
            tasks: self.tasks.clone(),
        };
        let refreshed = refresh_one(start, &self.stacks, &sub, task_id, event_id)?;
        self.absorb(&refreshed.decision);
        Ok(())
    }

    /// `apply_tracker_status`: the task moves to the status its issue requires. A close as done
    /// needs the verification service's external acceptance, which is not ported: that case is
    /// reported in the errors and the task stays where it is.
    fn apply_tracker_status(
        &mut self,
        task: &Task,
        row: &ExternalItem,
        changed: &mut Vec<&'static str>,
    ) -> Result<(), WgError> {
        let cancelled: Vec<&str> = self.cancelled.iter().map(String::as_str).collect();
        let target = tracker_target_status(
            task.status,
            &row.tracker_status_category,
            row.tracker_resolution.as_deref(),
            &cancelled,
        );
        let Some(target) = target else {
            return Ok(());
        };
        if target == TaskStatus::Accepted {
            self.counts.errors.push(format!(
                "{}: closing it from {} as done needs external acceptance, which is not ported yet",
                task.key, self.connection
            ));
            return Ok(());
        }
        let reason = format!("external:{}: {}", self.connection, row.tracker_status);
        let mut extra = JsonObject::new();
        extra.insert("tracker".to_owned(), Value::String(self.connection.clone()));
        extra.insert(
            "tracker_status".to_owned(),
            Value::String(row.tracker_status.clone()),
        );
        let sub = self.next_stamp();
        let moved = transition(
            &self.tasks,
            &self.stacks,
            &sub,
            sub.ids(),
            task.id,
            target,
            Change {
                reason: Some(&reason),
                privileged: true,
                tracker: true,
                caused_by: None,
                attempt_id: None,
                extra: Some(&extra),
            },
        )?;
        self.absorb(&moved.decision);
        changed.push("status");
        Ok(())
    }

    /// One reconcile action on a tracked item.
    fn reconcile_one(&mut self, item: &ExternalItem, action: ReconcileAction) {
        match action {
            ReconcileAction::Untouched => {}
            ReconcileAction::Restored {
                clear_missing,
                unarchive,
            } => {
                let Ok(task) = self.task(item.task_id) else {
                    return;
                };
                if clear_missing {
                    self.write(Write::update(ExternalItem {
                        missing_since: None,
                        ..item.clone()
                    }));
                }
                if unarchive && task.archived_at.is_some() {
                    self.unarchive(&task);
                }
            }
            ReconcileAction::Missing { since, archive } => {
                self.missing += 1;
                if item.missing_since != Some(since) {
                    self.write(Write::update(ExternalItem {
                        missing_since: Some(since),
                        ..item.clone()
                    }));
                }
                if archive {
                    let Ok(task) = self.task(item.task_id) else {
                        return;
                    };
                    let next = Task {
                        archived_at: Some(self.stamp.at),
                        version: task.version + 1,
                        ..task.clone()
                    };
                    self.write(Write::update(next));
                    self.event(
                        EventPayload::TaskArchived(TaskKeyed {
                            key: task.key.clone(),
                        }),
                        task.id,
                        None,
                    );
                    self.archived += 1;
                }
            }
        }
    }

    /// The `updated` cursor write of a page.
    fn cursor(&mut self, existing: Option<&SystemCursor>, name: &str, candidate: i64) {
        if let Some(write) = cursor_write(existing, name, candidate, self.stamp.at) {
            self.write(write);
        }
    }
}

/// Stores the snapshot on its tracked item (`_store_item`).
fn store_item(
    row: &mut ExternalItem,
    connection: &str,
    snap: &TrackerSnapshot,
    digest: &str,
    now: Timestamp,
) {
    row.connection = connection.to_owned();
    row.external_key = snap.key.clone();
    row.url = snap.url.clone();
    row.tracker_status = snap.status_name.clone();
    row.tracker_status_category = snap.status_category.as_str().to_owned();
    row.tracker_resolution = snap.resolution.clone();
    row.remote_updated_at = Some(snap.remote_updated_at);
    row.snapshot = snap.raw.as_object().cloned().unwrap_or_default();
    row.snapshot_hash = digest.to_owned();
    row.synced_at = Some(now);
    row.missing_since = None;
}

/// A tracked item row before `store_item` fills it in.
fn blank_item(
    id: Id,
    workspace_id: Id,
    task_id: Id,
    connection: &str,
    external_id: &str,
    now: Timestamp,
) -> ExternalItem {
    ExternalItem {
        id,
        workspace_id,
        task_id,
        connection: connection.to_owned(),
        external_id: external_id.to_owned(),
        external_key: String::new(),
        url: String::new(),
        tracker_status: String::new(),
        tracker_status_category: String::new(),
        tracker_resolution: None,
        remote_updated_at: None,
        snapshot: JsonObject::new(),
        snapshot_hash: String::new(),
        synced_at: None,
        missing_since: None,
        created_at: now,
        updated_at: now,
    }
}

/// The stored link as the pure plan compares it.
fn stored_link(link: &TaskLink) -> StoredLink {
    StoredLink {
        normalized_url: link.normalized_url.clone(),
        created_by: link.created_by.clone(),
        values: LinkValues {
            url: link.url.clone(),
            provider: link.provider.clone(),
            repo_full_name: link.repo_full_name.clone(),
            number: link.number,
            repository_id: link.repository_id.map(|id| id.to_string()),
            sources: link.sources.iter().cloned().collect(),
            title: link.title.clone(),
        },
    }
}

/// A new pull request link row, created by the sync.
fn new_link(
    id: Id,
    task_id: Id,
    normalized_url: String,
    values: &LinkValues,
    at: Timestamp,
) -> TaskLink {
    TaskLink {
        id,
        task_id,
        kind: PULL_REQUEST_KIND.to_owned(),
        provider: values.provider.clone(),
        url: values.url.clone(),
        normalized_url,
        repo_full_name: values.repo_full_name.clone(),
        number: values.number,
        repository_id: values
            .repository_id
            .as_deref()
            .and_then(|text| parse_id(text).ok()),
        sources: values.sources.iter().cloned().collect(),
        title: values.title.clone(),
        created_by: SYNC_ACTOR.to_owned(),
        created_at: at,
        updated_at: at,
    }
}

/// The field names a field update changes, in the Python order.
fn update_names(updates: &FieldUpdates) -> Vec<&'static str> {
    [
        updates.title.is_some().then_some("title"),
        updates.description.is_some().then_some("description"),
        updates.priority.is_some().then_some("priority"),
        updates.assignee.is_some().then_some("assignee"),
        (updates.clear_parent || updates.parent_key.is_some()).then_some("parent"),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}
