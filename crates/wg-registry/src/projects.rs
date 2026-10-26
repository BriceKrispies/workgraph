//! Projects as decisions (`ProjectService` in `core/services/projects.py`): create, update and
//! the repository memberships.
//!
//! Each decision reads a [`RegistrySlice`] of the project's workspace (its projects, their
//! memberships and its repositories, archived ones included) and replies with the project after
//! the change and its repository ids, which is what `ProjectOut` shows.

use std::collections::BTreeSet;

use rpds::VectorSync;
use serde_json::Value;
use wg_model::event::{
    Changes, ProjectCreated, ProjectRepositoryAdded, ProjectRepositoryRemoved, ProjectUpdated,
};
use wg_model::{
    Decision, EventDraft, EventPayload, JsonObject, Project, ProjectRepository, RegistrySlice,
    Stamp, Write, py_repr_str,
};
use wg_types::{ErrorCode, Id, Permission, WgError};

use crate::resolve;

/// A project with its repository ids, sorted: `ProjectOut`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectView {
    pub project: Project,
    pub repository_ids: Vec<Id>,
}

/// The project as `slice` holds it once `writes` are applied.
fn view_after(
    slice: &RegistrySlice,
    project_id: Id,
    writes: &VectorSync<Write>,
) -> Result<ProjectView, WgError> {
    let after = slice.apply_all(writes.iter());
    let ids: BTreeSet<Id> = after
        .project_repositories_of(project_id)
        .map(|link| link.repository_id)
        .collect();
    after
        .project(project_id)
        .cloned()
        .map(|project| ProjectView {
            project,
            repository_ids: ids.into_iter().collect(),
        })
        .ok_or_else(|| WgError::new(ErrorCode::WorkgraphError, "the project is not loaded"))
}

/// A decision whose reply is the project the writes leave behind.
fn replying(
    slice: &RegistrySlice,
    project_id: Id,
    decision: Decision<()>,
) -> Result<Decision<ProjectView>, WgError> {
    let view = view_after(slice, project_id, &decision.writes)?;
    Ok(decision.with_reply(view))
}

/// The project a reference names in the workspace, with its repository ids (the reads).
pub fn view(
    slice: &RegistrySlice,
    workspace_id: Id,
    reference: &str,
) -> Result<ProjectView, WgError> {
    let found = resolve::project(slice, workspace_id, reference)?;
    view_after(slice, found.id, &VectorSync::new_sync())
}

fn text(dump: &JsonObject, key: &str) -> String {
    dump.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn event(
    stamp: &Stamp,
    id: Id,
    workspace_id: Id,
    project_id: Id,
    payload: EventPayload,
) -> EventDraft {
    EventDraft::new(stamp, id, Some(workspace_id), project_id, payload)
}

/// `ProjectService.create` from a validated `ProjectCreate` dump: the project, a membership for
/// each named repository (an archived one is not found) and `project.created`.
pub fn create(
    slice: &RegistrySlice,
    stamp: &Stamp,
    workspace_id: Id,
    dump: &JsonObject,
) -> Result<Decision<ProjectView>, WgError> {
    stamp.require(&[Permission::ProjectWrite])?;
    let workspace = slice
        .workspace(workspace_id)
        .ok_or_else(|| WgError::new(ErrorCode::NotFound, "workspace not found"))?;
    let slug = text(dump, "slug");
    let taken = slice
        .projects
        .values()
        .any(|project| project.workspace_id == workspace_id && project.slug == slug);
    if taken {
        return Err(WgError::new(
            ErrorCode::Conflict,
            format!(
                "project {} already exists in workspace {}",
                py_repr_str(&slug),
                workspace.slug
            ),
        ));
    }
    let ([project_id, event_id], _) = stamp.ids().take::<2>();
    let project = Project {
        id: project_id,
        workspace_id,
        slug,
        name: text(dump, "name"),
        description: text(dump, "description"),
        status: "active".to_owned(),
        settings: dump
            .get("settings")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        created_at: stamp.at,
        updated_at: stamp.at,
    };
    let repository_ids = dump
        .get("repositories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|reference| {
            resolve::repository(slice, workspace_id, reference, false).map(|repo| repo.id)
        })
        .collect::<Result<Vec<Id>, WgError>>()?;
    let created = EventPayload::ProjectCreated(ProjectCreated {
        slug: project.slug.clone(),
        name: project.name.clone(),
        repository_ids: repository_ids.iter().copied().collect(),
    });
    let decision = repository_ids.iter().fold(
        Decision::new(()).write(Write::insert(project)),
        |decision, repository_id| {
            decision.write(Write::insert(ProjectRepository {
                project_id,
                repository_id: *repository_id,
                role: "member".to_owned(),
                access_mode: None,
                created_at: stamp.at,
            }))
        },
    );
    replying(
        slice,
        project_id,
        decision.event(event(stamp, event_id, workspace_id, project_id, created)),
    )
}

/// The fields a `ProjectUpdate` can change, in the order Python compares them.
const UPDATABLE: [&str; 5] = ["slug", "name", "description", "status", "settings"];

fn current(project: &Project, field: &str) -> Value {
    match field {
        "slug" => Value::String(project.slug.clone()),
        "name" => Value::String(project.name.clone()),
        "description" => Value::String(project.description.clone()),
        "status" => Value::String(project.status.clone()),
        _ => Value::Object(project.settings.clone()),
    }
}

fn with_field(project: Project, field: &str, value: &Value) -> Project {
    let words = value.as_str().unwrap_or_default().to_owned();
    match field {
        "slug" => Project {
            slug: words,
            ..project
        },
        "name" => Project {
            name: words,
            ..project
        },
        "description" => Project {
            description: words,
            ..project
        },
        "status" => Project {
            status: words,
            ..project
        },
        _ => Project {
            settings: value.as_object().cloned().unwrap_or_default(),
            ..project
        },
    }
}

/// `ProjectService.update` from a validated `ProjectUpdate` dump: every field that is given (not
/// null) and differs is changed, and one `project.updated` event lists the changes.
pub fn update(
    slice: &RegistrySlice,
    stamp: &Stamp,
    workspace_id: Id,
    reference: &str,
    dump: &JsonObject,
) -> Result<Decision<ProjectView>, WgError> {
    stamp.require(&[Permission::ProjectWrite])?;
    let found = resolve::project(slice, workspace_id, reference)?;
    let new_slug = dump
        .get("slug")
        .and_then(Value::as_str)
        .filter(|slug| *slug != found.slug);
    if let Some(slug) = new_slug {
        let taken = slice
            .projects
            .values()
            .any(|project| project.workspace_id == workspace_id && project.slug == slug);
        if taken {
            return Err(WgError::new(
                ErrorCode::Conflict,
                format!(
                    "project slug {} is already used in this workspace",
                    py_repr_str(slug)
                ),
            ));
        }
    }
    let (row, changes) = UPDATABLE.iter().fold(
        (found.clone(), Changes::new()),
        |(row, changes), field| match dump.get(*field) {
            Some(value) if !value.is_null() && current(&row, field) != *value => {
                let before = current(&row, field);
                (
                    with_field(row, field, value),
                    changes.with(field, before, value.clone()),
                )
            }
            _ => (row, changes),
        },
    );
    if changes.is_empty() {
        return replying(slice, found.id, Decision::new(()));
    }
    let ([event_id], _) = stamp.ids().take::<1>();
    let row = Project {
        updated_at: stamp.at,
        ..row
    };
    let payload = EventPayload::ProjectUpdated(ProjectUpdated { changes });
    replying(
        slice,
        found.id,
        Decision::new(()).write(Write::update(row)).event(event(
            stamp,
            event_id,
            workspace_id,
            found.id,
            payload,
        )),
    )
}

/// `ProjectService.add_repository`: a membership with `role`; one that exists is left as it is.
pub fn add_repository(
    slice: &RegistrySlice,
    stamp: &Stamp,
    workspace_id: Id,
    reference: &str,
    repository: &str,
    role: &str,
) -> Result<Decision<ProjectView>, WgError> {
    stamp.require(&[Permission::ProjectWrite])?;
    let project = resolve::project(slice, workspace_id, reference)?;
    let repo = resolve::repository(slice, workspace_id, repository, false)?;
    if slice
        .project_repositories
        .contains_key(&(project.id, repo.id))
    {
        return replying(slice, project.id, Decision::new(()));
    }
    let ([event_id], _) = stamp.ids().take::<1>();
    let payload = EventPayload::ProjectRepositoryAdded(ProjectRepositoryAdded {
        repository_id: repo.id,
        role: role.to_owned(),
    });
    replying(
        slice,
        project.id,
        Decision::new(())
            .write(Write::insert(ProjectRepository {
                project_id: project.id,
                repository_id: repo.id,
                role: role.to_owned(),
                access_mode: None,
                created_at: stamp.at,
            }))
            .event(event(stamp, event_id, workspace_id, project.id, payload)),
    )
}

/// `ProjectService.remove_repository`: removes the membership of a repository, archived or not;
/// a repository that was not a member changes nothing.
pub fn remove_repository(
    slice: &RegistrySlice,
    stamp: &Stamp,
    workspace_id: Id,
    reference: &str,
    repository: &str,
) -> Result<Decision<ProjectView>, WgError> {
    stamp.require(&[Permission::ProjectWrite])?;
    let project = resolve::project(slice, workspace_id, reference)?;
    let repo = resolve::repository(slice, workspace_id, repository, true)?;
    let ([event_id], _) = stamp.ids().take::<1>();
    let removal = slice
        .project_repositories
        .get(&(project.id, repo.id))
        .map(|link| {
            let payload = EventPayload::ProjectRepositoryRemoved(ProjectRepositoryRemoved {
                repository_id: repo.id,
            });
            Decision::new(())
                .write(Write::delete(link.row_key()))
                .event(event(stamp, event_id, workspace_id, project.id, payload))
        });
    replying(
        slice,
        project.id,
        removal.unwrap_or_else(|| Decision::new(())),
    )
}
