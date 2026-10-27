//! Pure: `verify config`, the creation or update of a verification configuration
//! (`VerificationService.create_config` and `_scope_id`).
//!
//! The decision validates every check, resolves the scope through the registry (a project by id or
//! slug, a repository or a task by id, name or key), resolves the repository each check names, and
//! writes the configuration row of that scope and name: a new row, or the existing one with its
//! checks replaced, its version bumped, its approver set and its `enabled` flag back on.

use serde_json::Value as Json;
use wg_json::Value;
use wg_model::entity::{JsonObject, VerificationConfig};
use wg_model::event::{EventDraft, EventPayload, VerificationConfigWritten};
use wg_model::stamp::Stamp;
use wg_model::{Decision, TaskSlice, Write, py_repr_str};
use wg_registry::resolve;
use wg_types::{ErrorCode, Id, Permission, WgError};
use wg_verify::CheckSpec;
use wg_wire::dto::verification::config_from_model;

/// What the write reads: the registry and tasks of the workspace, and its configurations.
#[derive(Debug, Clone, Default)]
pub struct ConfigWorld {
    pub tasks: TaskSlice,
    pub configs: Vec<VerificationConfig>,
}

/// The arguments of `verify config`.
#[derive(Debug, Clone, Copy)]
pub struct ConfigRequest<'a> {
    pub scope_type: &'a str,
    pub scope: Option<&'a str>,
    pub name: &'a str,
    pub checks: &'a [Json],
}

fn invalid(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}

/// The check validated as the `CheckSpec` model validates it.
fn spec_of(raw: &Json) -> Result<CheckSpec, WgError> {
    match raw {
        Json::Object(object) => CheckSpec::from_json(object),
        _ => Err(invalid("check: expected an object")),
    }
}

/// `_scope_id`: the id of the scope the configuration applies to.
fn scope_id(tasks: &TaskSlice, workspace: Id, request: &ConfigRequest<'_>) -> Result<Id, WgError> {
    if request.scope_type == "workspace" {
        return Ok(workspace);
    }
    let reference = request.scope.ok_or_else(|| {
        invalid(format!(
            "scope reference required for scope type {}",
            request.scope_type
        ))
    })?;
    match request.scope_type {
        "project" => Ok(resolve::project(&tasks.registry, workspace, reference)?.id),
        "repository" => Ok(resolve::repository(&tasks.registry, workspace, reference, false)?.id),
        "task" => Ok(resolve::task(tasks, Some(workspace), reference)?.id),
        other => Err(invalid(format!(
            "unknown scope type {}",
            py_repr_str(other)
        ))),
    }
}

/// The configuration after an update: new checks, next version, this approver, enabled.
fn updated(
    existing: &VerificationConfig,
    checks: &[JsonObject],
    stamp: &Stamp,
) -> VerificationConfig {
    VerificationConfig {
        checks: checks.iter().cloned().collect(),
        version: existing.version + 1,
        approved_by: stamp.actor.to_string(),
        enabled: true,
        updated_at: stamp.at,
        ..existing.clone()
    }
}

/// `create_config`: writes the configuration and replies with it (`VerificationConfigOut`).
pub fn write_config(
    world: &ConfigWorld,
    stamp: &Stamp,
    workspace: Id,
    request: &ConfigRequest<'_>,
) -> Result<Decision<Value>, WgError> {
    stamp.require(&[Permission::ConfigManage])?;
    let specs = request
        .checks
        .iter()
        .map(spec_of)
        .collect::<Result<Vec<_>, _>>()?;
    if specs.is_empty() {
        return Err(invalid(
            "a verification configuration needs at least one check",
        ));
    }
    let names: std::collections::BTreeSet<&str> =
        specs.iter().map(|spec| spec.name.as_str()).collect();
    if names.len() != specs.len() {
        return Err(invalid("check names must be unique within a configuration"));
    }
    let scope = scope_id(&world.tasks, workspace, request)?;
    specs
        .iter()
        .filter_map(|spec| spec.repository.as_deref())
        .try_for_each(|repository| {
            resolve::repository(&world.tasks.registry, workspace, repository, false).map(|_| ())
        })?;
    let payload: Vec<JsonObject> = specs.iter().map(CheckSpec::to_json).collect();
    let existing = world.configs.iter().find(|config| {
        config.workspace_id == workspace
            && config.scope_type == request.scope_type
            && config.scope_id == scope
            && config.name == request.name
    });
    let ([config_id, event_id], _) = stamp.ids().take::<2>();
    let (config, write) = match existing {
        None => {
            let created = VerificationConfig {
                id: config_id,
                workspace_id: workspace,
                scope_type: request.scope_type.to_owned(),
                scope_id: scope,
                name: request.name.to_owned(),
                checks: payload.iter().cloned().collect(),
                enabled: true,
                approved_by: stamp.actor.to_string(),
                version: 1,
                created_at: stamp.at,
                updated_at: stamp.at,
            };
            (created.clone(), Write::insert(created))
        }
        Some(found) => {
            let next = updated(found, &payload, stamp);
            (next.clone(), Write::update(next))
        }
    };
    let data = VerificationConfigWritten {
        checks: Json::Array(payload.into_iter().map(Json::Object).collect()),
        name: request.name.to_owned(),
        scope_id: scope,
        scope_type: request.scope_type.to_owned(),
    };
    let payload = if existing.is_none() {
        EventPayload::VerificationConfigCreated(data)
    } else {
        EventPayload::VerificationConfigUpdated(data)
    };
    let recorded = EventDraft::new(stamp, event_id, Some(workspace), config.id, payload);
    Ok(Decision::new(config_from_model(&config))
        .write(write)
        .event(recorded))
}
