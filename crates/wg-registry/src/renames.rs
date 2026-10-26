//! The two record updates `project rename-term` applies besides the project and its tasks: a
//! repository's name and description, and a verification config's checks.
//!
//! These are the parts of `RepositoryService.update` and `VerificationService.create_config` that a
//! rename reaches; the full services belong to the repository and verification groups.

use rpds::VectorSync;
use serde_json::Value;
use wg_model::entity::VerificationConfig;
use wg_model::event::{Changes, RepositoryUpdated, VerificationConfigWritten};
use wg_model::{
    Decision, EventDraft, EventPayload, JsonObject, RegistrySlice, Repository, Stamp, Write,
    py_repr_str,
};
use wg_types::{ErrorCode, Id, Permission, WgError};

use crate::resolve;

/// `RepositoryService.update` with a new `name` and/or `description`: a name used by another
/// repository of the workspace (archived ones count) is a conflict.
pub fn rename_repository(
    slice: &RegistrySlice,
    stamp: &Stamp,
    workspace_id: Id,
    repository_id: Id,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<Decision<()>, WgError> {
    stamp.require(&[Permission::RepoAdmin])?;
    let found = resolve::repository(slice, workspace_id, &repository_id.to_string(), false)?;
    let taken = name.is_some_and(|name| {
        name != found.name
            && slice
                .repositories
                .values()
                .any(|repo| repo.workspace_id == workspace_id && repo.name == name)
    });
    if let Some(name) = name.filter(|_| taken) {
        return Err(WgError::new(
            ErrorCode::Conflict,
            format!(
                "repository name {} is already used in this workspace",
                py_repr_str(name)
            ),
        ));
    }
    let changes = [
        ("name", name.filter(|name| *name != found.name), &found.name),
        (
            "description",
            description.filter(|text| *text != found.description),
            &found.description,
        ),
    ]
    .into_iter()
    .fold(
        Changes::new(),
        |changes, (field, after, before)| match after {
            Some(after) => changes.with(
                field,
                Value::String(before.clone()),
                Value::String(after.to_owned()),
            ),
            None => changes,
        },
    );
    if changes.is_empty() {
        return Ok(Decision::new(()));
    }
    let row = Repository {
        name: name.map_or_else(|| found.name.clone(), str::to_owned),
        description: description.map_or_else(|| found.description.clone(), str::to_owned),
        updated_at: stamp.at,
        ..found.clone()
    };
    let ([event_id], _) = stamp.ids().take::<1>();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(workspace_id),
        found.id,
        EventPayload::RepositoryUpdated(RepositoryUpdated { changes }),
    );
    Ok(Decision::new(()).write(Write::update(row)).event(event))
}

/// `VerificationService.create_config` for a config that exists: its checks replaced, its version
/// raised, approved by the actor and enabled. Every repository a check names must resolve.
pub fn resave_config(
    slice: &RegistrySlice,
    stamp: &Stamp,
    config: &VerificationConfig,
    checks: &[JsonObject],
) -> Result<Decision<()>, WgError> {
    stamp.require(&[Permission::ConfigManage])?;
    let names: Vec<&str> = checks
        .iter()
        .filter_map(|check| check.get("name").and_then(Value::as_str))
        .collect();
    if checks.is_empty() {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "a verification configuration needs at least one check",
        ));
    }
    if names
        .iter()
        .any(|name| names.iter().filter(|n| *n == name).count() > 1)
    {
        return Err(WgError::new(
            ErrorCode::ValidationFailed,
            "check names must be unique within a configuration",
        ));
    }
    checks
        .iter()
        .filter_map(|check| check.get("repository").and_then(Value::as_str))
        .try_for_each(|reference| {
            resolve::repository(slice, config.workspace_id, reference, false).map(|_| ())
        })?;
    let row = VerificationConfig {
        checks: checks.iter().cloned().collect::<VectorSync<_>>(),
        version: config.version + 1,
        approved_by: stamp.actor.to_string(),
        enabled: true,
        updated_at: stamp.at,
        ..config.clone()
    };
    let ([event_id], _) = stamp.ids().take::<1>();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(config.workspace_id),
        config.id,
        EventPayload::VerificationConfigUpdated(VerificationConfigWritten {
            checks: Value::Array(checks.iter().cloned().map(Value::Object).collect()),
            name: config.name.clone(),
            scope_id: config.scope_id,
            scope_type: config.scope_type.clone(),
        }),
    );
    Ok(Decision::new(()).write(Write::update(row)).event(event))
}
