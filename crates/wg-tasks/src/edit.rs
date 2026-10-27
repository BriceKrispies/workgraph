//! The edit decisions of a task update: the update request model, its validation and reading, and
//! the update decision over the task world (`TaskService.update`), with the enrichment a text change
//! queues. Pure: the feature's service and the registry rename both run them.

use crate::accept::substamp;
use crate::files::FileInput;
use crate::requests::{component, run};
use crate::shape::task_out;
use crate::transition::{Applied, Change, transition};
use crate::update::{TaskPatch, update_plan};
use serde_json::{Map, Value, Value as Json, json};
use wg_jobs::queue::{NewJob, enqueue};
use wg_model::RowKey;
use wg_model::entity::TaskProject;
use wg_model::entity::{Task, TaskFile, TaskTag};
use wg_model::event::{EventDraft, EventPayload};
use wg_model::slice::QueueSlice;
use wg_model::stamp::Stamp;
use wg_model::{Decision, Write};
use wg_registry::resolve;
use wg_storecodec::task_world_sql::TaskWorld;
use wg_types::Permission;
use wg_types::pyval::flag;
use wg_types::{Id, TaskPriority, TaskStatus};
use wg_types::{Timestamp, WgError};

pub const UPDATE_ORDER: [&str; 22] = [
    "expected_version",
    "title",
    "description",
    "acceptance_criteria",
    "status",
    "priority",
    "tags",
    "add_tags",
    "remove_tags",
    "files",
    "add_files",
    "projects",
    "parent",
    "clear_parent",
    "assignee",
    "assigned_agent",
    "auto_execute",
    "due_at",
    "scheduled_at",
    "estimated_effort",
    "metadata",
    "reason",
];

/// Validates a `TaskUpdate` body.
pub fn validate_update(body: &Value) -> Result<Map<String, Value>, WgError> {
    run(&component("TaskUpdate", &UPDATE_ORDER)?, body)
}

/// A validated `TaskUpdate`, with references still as typed. A nullable field the request did
/// not name is `None`; one it named is `Some`, with `Some(None)` clearing it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateRead {
    pub expected_version: Option<i64>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub acceptance_criteria: Option<Vec<String>>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub tags: Option<Vec<String>>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub files: Option<Vec<FileInput>>,
    pub add_files: Vec<FileInput>,
    pub projects: Option<Vec<String>>,
    pub parent: Option<String>,
    pub clear_parent: bool,
    pub assignee: Option<Option<String>>,
    pub assigned_agent: Option<Option<String>>,
    pub auto_execute: Option<bool>,
    pub due_at: Option<Option<Timestamp>>,
    pub scheduled_at: Option<Option<Timestamp>>,
    pub estimated_effort: Option<Map<String, Value>>,
    pub metadata: Option<Map<String, Value>>,
    pub reason: Option<String>,
}

/// Reads a validated `TaskUpdate`; `given` is the request body, which tells a field that was
/// left out from one that was set to null (`model_dump(exclude_unset=True)`).
pub fn read_update(
    dump: &Map<String, Value>,
    given: &Map<String, Value>,
) -> Result<UpdateRead, WgError> {
    let named = |key: &str| given.contains_key(key);
    let clearable =
        |key: &str| -> Option<Option<String>> { named(key).then(|| string_of(dump, key)) };
    let clearable_time = |key: &str| -> Result<Option<Option<Timestamp>>, WgError> {
        if named(key) {
            time_of(dump, key).map(Some)
        } else {
            Ok(None)
        }
    };
    Ok(UpdateRead {
        expected_version: dump.get("expected_version").and_then(Value::as_i64),
        title: string_of(dump, "title"),
        description: string_of(dump, "description"),
        acceptance_criteria: strings_of(dump, "acceptance_criteria"),
        status: string_of(dump, "status").and_then(|text| TaskStatus::parse(&text)),
        priority: string_of(dump, "priority").and_then(|text| TaskPriority::parse(&text)),
        tags: strings_of(dump, "tags"),
        add_tags: strings_of(dump, "add_tags").unwrap_or_default(),
        remove_tags: strings_of(dump, "remove_tags").unwrap_or_default(),
        files: files_of(dump, "files"),
        add_files: files_of(dump, "add_files").unwrap_or_default(),
        projects: strings_of(dump, "projects"),
        parent: string_of(dump, "parent"),
        clear_parent: flag(dump.get("clear_parent")),
        assignee: clearable("assignee"),
        assigned_agent: clearable("assigned_agent"),
        auto_execute: dump.get("auto_execute").and_then(Value::as_bool),
        due_at: clearable_time("due_at")?,
        scheduled_at: clearable_time("scheduled_at")?,
        estimated_effort: object_of(dump, "estimated_effort"),
        metadata: object_of(dump, "metadata"),
        reason: string_of(dump, "reason"),
    })
}

pub fn time_of(dump: &Map<String, Value>, key: &str) -> Result<Option<Timestamp>, WgError> {
    match dump.get(key).and_then(Value::as_str) {
        // A naive datetime counts as UTC (Python keeps it naive; the stored instant is the same).
        Some(text) => Timestamp::parse_iso(text)
            .or_else(|_| Timestamp::parse_iso(&format!("{text}Z")))
            .map(Some),
        None => Ok(None),
    }
}

pub fn strings_of(dump: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    dump.get(key).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()
    })
}

/// Whether a datetime field holds a naive value: text that only parses once `Z` is appended.
pub fn is_naive(dump: &Map<String, Value>, key: &str) -> bool {
    dump.get(key)
        .and_then(Value::as_str)
        .is_some_and(|text| Timestamp::parse_iso(text).is_err())
}

pub fn files_of(dump: &Map<String, Value>, key: &str) -> Option<Vec<FileInput>> {
    dump.get(key).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .map(|item| FileInput {
                path: item["path"].as_str().unwrap_or_default().to_owned(),
                anchor: item["anchor"].as_str().map(str::to_owned),
                note: item["note"].as_str().unwrap_or_default().to_owned(),
            })
            .collect()
    })
}

/// Whether a text change queues reclassification and re-embedding (`llm.auto_classify` and
/// `llm.auto_embed`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Enrichment {
    pub classify: bool,
    pub embed: bool,
}

/// `config.llm.auto_classify` and `auto_embed` (both default on) from the configuration.
#[must_use]
pub fn enrichment_from(config: &serde_json::Map<String, Json>) -> Enrichment {
    let llm = config.get("llm").and_then(Json::as_object);
    let flag = |key: &str| {
        llm.and_then(|section| section.get(key))
            .and_then(Json::as_bool)
            .unwrap_or(true)
    };
    Enrichment {
        classify: flag("auto_classify"),
        embed: flag("auto_embed"),
    }
}

/// `TaskService._enqueue_enrichment`: the classify and embed jobs of a material change. `queue`
/// holds the jobs already stored for the task: an enqueue under an idempotency key that a stored
/// job holds gives that job back and queues nothing (`JobQueue.enqueue`).
pub fn enrichment_jobs(
    stamp: &Stamp,
    task: &Task,
    caused_by: Id,
    enrich: Enrichment,
    queue: &QueueSlice,
) -> Decision<()> {
    let mut decision = Decision::new(());
    let mut salt = 0;
    let mut add = |decision: Decision<()>, kind: &str, payload: Json, key: String| {
        let mut job = NewJob::new(kind);
        job.payload = payload.as_object().cloned().unwrap_or_default();
        job.workspace_id = Some(task.workspace_id);
        job.task_id = Some(task.id);
        job.idempotency_key = Some(key);
        job.resource_keys = vec!["gpu".to_owned()];
        job.max_attempts = 4;
        job.backoff_base_seconds = 15.0;
        job.correlation_id = Some(stamp.correlation.clone());
        job.causation_id = Some(caused_by);
        job.created_by = stamp.actor.to_string();
        salt += 1;
        let queued = enqueue(queue, &job, &substamp(stamp, salt));
        queued
            .writes
            .iter()
            .fold(decision, |acc, write| acc.write(write.clone()))
    };
    if enrich.classify {
        decision = add(
            decision,
            "classify_task",
            json!({"task_id": task.id.to_string(), "reason": "material_change"}),
            format!("classify:{}:{}", task.id, task.description_hash),
        );
    }
    if enrich.embed {
        decision = add(
            decision,
            "embed_task",
            json!({"task_id": task.id.to_string()}),
            format!("embed:{}:{}", task.id, task.description_hash),
        );
    }
    decision
}

/// Appends `other`'s writes, events and effects to `main`.
pub fn absorb(main: Decision<()>, other: Decision<()>) -> Decision<()> {
    main.then(other)
}

/// `TaskService.update`: the update of one task, resolved from `reference`. `queue` holds the jobs
/// stored for that task (see [`enrichment_jobs`]).
pub fn update_task(
    world: &TaskWorld,
    queue: &QueueSlice,
    stamp: &Stamp,
    workspace: Option<Id>,
    reference: &str,
    read: UpdateRead,
    enrich: Enrichment,
) -> Result<Decision<wg_json::Value>, WgError> {
    stamp.require(&[Permission::TaskWrite])?;
    let tasks = &world.tasks;
    let task = resolve::task(tasks, workspace, reference)?;
    let projects = read
        .projects
        .as_ref()
        .map(|refs| {
            refs.iter()
                .map(|reference| {
                    resolve::project(&tasks.registry, task.workspace_id, reference).map(|p| p.id)
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    let parent = match (read.clear_parent && task.parent_id.is_some(), &read.parent) {
        (false, Some(reference)) => {
            Some(resolve::task(tasks, Some(task.workspace_id), reference)?.id)
        }
        _ => None,
    };
    let patch = TaskPatch {
        expected_version: read.expected_version,
        title: read.title,
        description: read.description,
        acceptance_criteria: read.acceptance_criteria,
        assignee: read.assignee,
        assigned_agent: read.assigned_agent,
        auto_execute: read.auto_execute,
        due_at: read.due_at,
        scheduled_at: read.scheduled_at,
        priority: read.priority,
        estimated_effort: read.estimated_effort,
        metadata: read.metadata,
        tags: read.tags,
        add_tags: read.add_tags,
        remove_tags: read.remove_tags,
        files: read.files,
        add_files: read.add_files,
        projects,
        parent,
        clear_parent: read.clear_parent,
        status: read.status,
        reason: read.reason,
    };
    let plan = update_plan(stamp, tasks, task, &patch)?;

    let before_tags: Vec<String> = tasks.tags_of(task.id).map(str::to_owned).collect();
    let before_files = tasks.files_of(task.id).count();
    let before_projects: Vec<Id> = tasks
        .projects_of(task.id)
        .map(|row| row.project_id)
        .collect();

    let mut decision = Decision::new(());
    if plan.task != *task {
        decision = decision.write(Write::update(plan.task.clone()));
    }
    let tag_order = plan.tags.clone();
    if let Some(after) = &plan.tags {
        for write in tag_writes(task, &before_tags, after) {
            decision = decision.write(write);
        }
    }
    if let Some(rows) = plan.files {
        for write in file_writes(task, before_files, rows) {
            decision = decision.write(write);
        }
    }
    if let Some(after) = &plan.projects {
        for write in project_writes(task, &before_projects, after) {
            decision = decision.write(write);
        }
    }
    let ids = stamp.ids();
    let (event_id, ids) = ids.next();
    let event_id = plan.event.as_ref().map(|_| event_id);
    if let (Some(payload), Some(id)) = (plan.event, event_id) {
        decision = decision.event(EventDraft::new(
            stamp,
            id,
            Some(task.workspace_id),
            task.id,
            EventPayload::TaskUpdated(payload),
        ));
    }
    let mut state = tasks.apply_all(decision.writes.iter());
    if let Some(target) = plan.status {
        let moved: Applied = transition(
            &state,
            &world.stacks,
            stamp,
            ids,
            task.id,
            target,
            Change {
                reason: patch.reason.as_deref(),
                caused_by: event_id,
                ..Change::default()
            },
        )?;
        state = moved.tasks;
        decision = absorb(decision, moved.decision);
    }
    let after = state
        .task(task.id)
        .ok_or_else(|| WgError::workgraph("the updated task vanished"))?;
    if let (true, true, Some(cause)) = (
        plan.material_change,
        enrich != Enrichment::default(),
        event_id,
    ) {
        decision = absorb(
            decision,
            enrichment_jobs(stamp, after, cause, enrich, queue),
        );
    }
    let out = task_out(&state, after, tag_order.as_deref());
    Ok(decision.with_reply(out))
}

pub fn string_of(dump: &Map<String, Value>, key: &str) -> Option<String> {
    dump.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub fn object_of(dump: &Map<String, Value>, key: &str) -> Option<Map<String, Value>> {
    dump.get(key).and_then(Value::as_object).cloned()
}

/// The rows that replace a task's tags: the old ones that are gone, the new ones that are not
/// there yet.
pub fn tag_writes(task: &Task, before: &[String], after: &[String]) -> Vec<Write> {
    let removed = before.iter().filter(|tag| !after.contains(tag)).map(|tag| {
        Write::delete(RowKey::TaskTag {
            task_id: task.id,
            tag: tag.clone(),
        })
    });
    let added = after.iter().filter(|tag| !before.contains(tag)).map(|tag| {
        Write::insert(TaskTag {
            task_id: task.id,
            tag: tag.clone(),
        })
    });
    removed.chain(added).collect()
}

pub fn file_writes(task: &Task, before: usize, rows: Vec<TaskFile>) -> Vec<Write> {
    (0..before)
        .map(|position| {
            Write::delete(RowKey::TaskFile {
                task_id: task.id,
                position: i64::try_from(position).unwrap_or(i64::MAX),
            })
        })
        .chain(rows.into_iter().map(Write::insert))
        .collect()
}

pub fn project_writes(task: &Task, before: &[Id], after: &[Id]) -> Vec<Write> {
    let removed = before.iter().filter(|id| !after.contains(id)).map(|id| {
        Write::delete(RowKey::TaskProject {
            task_id: task.id,
            project_id: *id,
        })
    });
    let added = after.iter().filter(|id| !before.contains(id)).map(|id| {
        Write::insert(TaskProject {
            task_id: task.id,
            project_id: *id,
        })
    });
    removed.chain(added).collect()
}
