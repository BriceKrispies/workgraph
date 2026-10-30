//! The pure half of the `workspace` and `project` commands (`cli/commands/core.py`, IF-03): the
//! request models, the outputs, and the rename of a term across a project.
//!
//! The decisions themselves are in `wg_registry` (`workspaces`, `projects`, `renames`); this
//! module validates what a command sends, shapes what it prints, and turns the loaded records of
//! a project into the inputs of `wg_tasks::rename::plan_rename` and the plan into updates.

use crate::spec::dto::workspace_summary::Counts;
use crate::spec::dto::{project, project_summary, workspace, workspace_summary};
use serde_json::{Map, Value as Json, json};
use wg_json::{Value, dict};
use wg_model::RegistrySlice;
use wg_model::entity::{Task, VerificationConfig, Workspace};
use wg_registry::projects::ProjectView;
use wg_rows::task_world_sql::TaskWorld;
use wg_tasks::rename::{
    ConfigRecord, FileRecord, ProjectChange, ProjectRecord, RenamePlan, RepositoryRecord,
    TaskChange, TaskRecord, Term,
};
use wg_types::{Id, TaskStatus, WgError};
use wg_wire::convention::{int, text};
use wg_wire::dto::task_brief;

use crate::spec::setup::py_title;
use wg_tasks::requests::{component, run};

/// `WorkspaceCreate`, validated: the dump in declaration order.
pub fn validate_workspace_create(body: &Json) -> Result<Map<String, Json>, WgError> {
    run(
        &component(
            "WorkspaceCreate",
            &["slug", "name", "description", "key_prefix"],
        )?,
        body,
    )
}

/// `ProjectCreate`, validated.
pub fn validate_project_create(body: &Json) -> Result<Map<String, Json>, WgError> {
    run(
        &component(
            "ProjectCreate",
            &["slug", "name", "description", "repositories", "settings"],
        )?,
        body,
    )
}

/// `ProjectUpdate`, validated.
pub fn validate_project_update(body: &Json) -> Result<Map<String, Json>, WgError> {
    run(
        &component(
            "ProjectUpdate",
            &["slug", "name", "description", "status", "settings"],
        )?,
        body,
    )
}

/// What `workspace create` sends: the name defaults to the title-cased slug.
#[must_use]
pub fn workspace_create_body(
    slug: &str,
    name: Option<&str>,
    prefix: &str,
    description: &str,
) -> Json {
    let name = name
        .filter(|name| !name.is_empty())
        .map_or_else(|| py_title(slug), str::to_owned);
    json!({"slug": slug, "name": name, "description": description, "key_prefix": prefix})
}

/// What `project create` sends: the name defaults to the title-cased slug with dashes as spaces.
#[must_use]
pub fn project_create_body(
    slug: &str,
    name: Option<&str>,
    description: &str,
    repositories: &[String],
) -> Json {
    let name = name
        .filter(|name| !name.is_empty())
        .map_or_else(|| py_title(&slug.replace('-', " ")), str::to_owned);
    json!({"slug": slug, "name": name, "description": description, "repositories": repositories})
}

/// What `project update` sends: only the options that were given.
#[must_use]
pub fn project_option_body(
    name: Option<&str>,
    description: Option<&str>,
    status: Option<&str>,
) -> Json {
    json!({"name": name, "description": description, "status": status})
}

/// `WorkspaceOut`.
#[must_use]
pub fn workspace_out(row: &Workspace) -> Value {
    workspace::from_model(row)
}

/// `ProjectOut`.
#[must_use]
pub fn project_out(view: &ProjectView) -> Value {
    project::from_model(&view.project, view.repository_ids.iter().copied())
}

/// The count of the `queued` or `dead` jobs among `(status, count)` pairs.
fn jobs_in(counts: &[(String, i64)], status: &str) -> i64 {
    counts
        .iter()
        .filter(|(name, _)| name == status)
        .map(|(_, count)| *count)
        .sum()
}

/// What a workspace summary counts, from the reads of the store.
#[must_use]
pub fn workspace_counts(
    projects: usize,
    repositories: usize,
    tasks_by_status: Vec<(String, i64)>,
    running_attempts: i64,
    jobs: &[(String, i64)],
) -> Counts {
    Counts {
        projects: i64::try_from(projects).unwrap_or(i64::MAX),
        repositories: i64::try_from(repositories).unwrap_or(i64::MAX),
        tasks_by_status,
        running_attempts,
        queued_jobs: jobs_in(jobs, "queued"),
        dead_jobs: jobs_in(jobs, "dead"),
    }
}

/// `WorkspaceSummary`.
#[must_use]
pub fn workspace_summary_out(row: &Workspace, counts: &Counts) -> Value {
    workspace_summary::from_model(row, counts)
}

/// `ProjectSummary`: the project, its repositories (by name), the live tasks by status and the
/// most recently updated ones.
#[must_use]
pub fn project_summary_out(
    view: &ProjectView,
    repositories: &[wg_model::entity::Repository],
    tasks_by_status: &[(String, i64)],
    recent: &[Task],
) -> Value {
    project_summary::from_model(
        project_out(view),
        repositories,
        tasks_by_status,
        recent.iter().map(task_brief::from_model).collect(),
    )
}

// ---------------------------------------------------------------- rename-term

/// The records `project rename-term` reads, as the plan's inputs.
#[derive(Debug, Clone)]
pub struct RenameInputs {
    pub project: ProjectRecord,
    pub repositories: Vec<RepositoryRecord>,
    pub configs: Vec<ConfigRecord>,
    pub tasks: Vec<TaskRecord>,
}

fn file_record(file: &wg_model::entity::TaskFile) -> FileRecord {
    FileRecord {
        path: file.path.clone(),
        anchor: file.anchor.clone(),
        note: file.note.clone(),
    }
}

/// Every task of the project, archived ones included, newest first (`sort="key"` descending).
fn project_tasks(world: &TaskWorld, project_id: Id) -> Vec<TaskRecord> {
    let slice = &world.tasks;
    let mut found: Vec<&Task> = slice
        .tasks
        .values()
        .filter(|task| {
            slice
                .projects_of(task.id)
                .any(|link| link.project_id == project_id)
        })
        .collect();
    found.sort_by_key(|task| std::cmp::Reverse(task.number));
    found
        .into_iter()
        .map(|task| TaskRecord {
            id: task.id,
            key: task.key.to_string(),
            status: task.status,
            archived: task.archived_at.is_some(),
            version: task.version,
            title: task.title.clone(),
            description: task.description.clone(),
            acceptance_criteria: task.acceptance_criteria.iter().cloned().collect(),
            files: slice.files_of(task.id).map(file_record).collect(),
            tags: slice.tags_of(task.id).map(str::to_owned).collect(),
        })
        .collect()
}

/// The plan's inputs from the loaded project, its repositories, the workspace's verification
/// configs and its tasks.
#[must_use]
pub fn rename_inputs(
    view: &ProjectView,
    registry: &RegistrySlice,
    configs: &[VerificationConfig],
    world: &TaskWorld,
) -> RenameInputs {
    RenameInputs {
        project: ProjectRecord {
            id: view.project.id,
            slug: view.project.slug.clone(),
            name: view.project.name.clone(),
            description: view.project.description.clone(),
        },
        repositories: view
            .repository_ids
            .iter()
            .filter_map(|id| registry.repository(*id))
            .map(|repo| RepositoryRecord {
                id: repo.id,
                name: repo.name.clone(),
                description: repo.description.clone(),
            })
            .collect(),
        configs: configs
            .iter()
            .map(|config| ConfigRecord {
                name: config.name.clone(),
                scope_type: config.scope_type.clone(),
                scope_id: config.scope_id,
                checks: Json::Array(config.checks.iter().cloned().map(Json::Object).collect()),
            })
            .collect(),
        tasks: project_tasks(world, view.project.id),
    }
}

/// The check objects of a config's `checks` value.
#[must_use]
pub fn checks_of(checks: &Json) -> Vec<Map<String, Json>> {
    checks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|check| check.as_object().cloned())
        .collect()
}

/// The `ProjectUpdate` body of a project rename: only the fields that change.
#[must_use]
pub fn project_update_body(change: &ProjectChange) -> Json {
    json!({"slug": change.slug, "name": change.name, "description": change.description})
}

/// The `TaskUpdate` body of a task rename: the version it was planned against, the reason and
/// the fields that change.
#[must_use]
pub fn task_update_body(change: &TaskChange, term: &Term) -> Json {
    let files = change.files.as_ref().map(|files| {
        files
            .iter()
            .map(|file| json!({"path": file.path, "anchor": file.anchor, "note": file.note}))
            .collect::<Vec<_>>()
    });
    let optional = [
        ("tags", change.tags.as_ref().map(|tags| json!(tags))),
        ("title", change.title.as_ref().map(|title| json!(title))),
        (
            "description",
            change.description.as_ref().map(|text| json!(text)),
        ),
        (
            "acceptance_criteria",
            change
                .acceptance_criteria
                .as_ref()
                .map(|items| json!(items)),
        ),
        ("files", files.map(Json::Array)),
    ];
    let mut body = Map::new();
    body.insert("expected_version".to_owned(), json!(change.version));
    body.insert(
        "reason".to_owned(),
        json!(format!("rename {} to {}", term.old(), term.new_word())),
    );
    optional
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key, value)))
        .for_each(|(key, value)| {
            body.insert(key.to_owned(), value);
        });
    Json::Object(body)
}

/// The rewritten absolute paths whose old spelling exists and new one does not, sorted and
/// without repeats (`_missing_paths`); `exists` is the shell's disk check.
#[must_use]
pub fn missing_paths(rewrites: &[(String, String)], exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    rewrites
        .iter()
        .filter(|(before, after)| exists(before) && !exists(after))
        .map(|(_, after)| after.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn status_text(status: TaskStatus) -> &'static str {
    status.as_str()
}

/// The report of `project rename-term`. `missing` holds each planned config's missing paths and
/// `errors` the `(target, message)` of every update that failed.
#[must_use]
pub fn rename_report(
    term: &Term,
    dry_run: bool,
    plan: &RenamePlan,
    missing: &[Vec<String>],
    errors: &[(String, String)],
) -> Value {
    let project = plan.project.as_ref().map_or(Value::Null, |change| {
        dict!({
            "slug": text(&change.old_slug),
            "new_slug": text(&change.new_slug),
            "name": text(&change.old_name),
            "new_name": text(&change.new_name),
        })
    });
    let repositories = plan.repositories.iter().map(|repo| {
        dict!({
            "name": text(&repo.old_name),
            "new_name": text(&repo.new_name),
            "description_sites": int(count(repo.description_sites)),
            "checkouts_unchanged": Value::Bool(true),
        })
    });
    let configs = plan
        .verification_configs
        .iter()
        .zip(missing)
        .map(|(config, paths)| {
            dict!({
                "name": text(&config.name),
                "sites": int(count(config.sites)),
                "missing_paths": Value::List(paths.iter().map(|path| text(path)).collect()),
            })
        });
    let tasks = plan.tasks.iter().map(|task| {
        dict!({
            "key": text(&task.key),
            "status": text(status_text(task.status)),
            "closed": Value::Bool(task.closed),
            "fields": dict(
                task.fields
                    .iter()
                    .map(|(name, sites)| (*name, int(count(*sites))))
                    .collect(),
            ),
        })
    });
    let failures = errors
        .iter()
        .map(|(target, error)| dict!({ "target": text(target), "error": text(error) }));
    dict!({
        "from": text(term.old()),
        "to": text(term.new_word()),
        "dry_run": Value::Bool(dry_run),
        "project": project,
        "repositories": Value::List(repositories.collect()),
        "verification_configs": Value::List(configs.collect()),
        "tasks": Value::List(tasks.collect()),
        "tags_renamed": int(count(plan.tags_renamed)),
        "closed_kept": int(count(plan.closed_kept)),
        "errors": Value::List(failures.collect()),
    })
}

fn count(sites: usize) -> i64 {
    i64::try_from(sites).unwrap_or(i64::MAX)
}
