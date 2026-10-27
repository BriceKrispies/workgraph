//! Workspace lifecycle as decisions (`WorkspaceService` in `core/services/workspaces.py`):
//! create, archive and the settings update.
//!
//! Each decision gets a [`RegistrySlice`] holding the workspaces and looks the reference up
//! itself, so a miss is the same `not_found` error Python raises. The reply of each is the
//! workspace row after the change, which the caller renders as `WorkspaceOut`.

use wg_model::event::{WorkspaceArchived, WorkspaceCreated, WorkspaceSettingsChanged};
use wg_model::{
    Decision, EventDraft, EventPayload, JsonObject, RegistrySlice, Stamp, Workspace, Write,
    py_repr_str,
};
use wg_types::{Permission, WgError};

use crate::{resolve, settings};
use wg_types::pyval::text;

/// The insert of a new workspace from a validated `WorkspaceCreate` dump, with its event, and
/// none of the checks of [`create`]. `init` uses it after its own existence test.
pub fn insert(stamp: &Stamp, dump: &JsonObject) -> Decision<Workspace> {
    let ([workspace_id, event_id], _) = stamp.ids().take::<2>();
    let row = Workspace {
        id: workspace_id,
        slug: text(dump.get("slug")).to_owned(),
        name: text(dump.get("name")).to_owned(),
        description: text(dump.get("description")).to_owned(),
        key_prefix: text(dump.get("key_prefix")).to_owned(),
        next_task_number: 1,
        settings: JsonObject::new(),
        archived_at: None,
        created_at: stamp.at,
        updated_at: stamp.at,
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(workspace_id),
        workspace_id,
        EventPayload::WorkspaceCreated(WorkspaceCreated {
            slug: row.slug.clone(),
            name: row.name.clone(),
            key_prefix: row.key_prefix.clone(),
        }),
    );
    Decision::new(row.clone())
        .write(Write::insert(row))
        .event(event)
}

/// `WorkspaceService.create`: a new workspace; a slug already taken (archived workspaces count)
/// is a conflict.
pub fn create(
    slice: &RegistrySlice,
    stamp: &Stamp,
    dump: &JsonObject,
) -> Result<Decision<Workspace>, WgError> {
    stamp.require(&[Permission::WorkspaceAdmin])?;
    let slug = text(dump.get("slug")).to_owned();
    if slice.workspace_by_slug(&slug).is_some() {
        return Err(WgError::conflict(format!(
            "workspace {} already exists",
            py_repr_str(&slug)
        )));
    }
    Ok(insert(stamp, dump))
}

/// `WorkspaceService.set_default`: the workspace the reference names. Recording it as the
/// configured default is the shell's, since the default lives in `config.toml`.
pub fn named(slice: &RegistrySlice, stamp: &Stamp, reference: &str) -> Result<Workspace, WgError> {
    stamp.require(&[Permission::WorkspaceAdmin])?;
    resolve::workspace(slice, reference).cloned()
}

/// `WorkspaceService.archive`: sets `archived_at`; the records are kept.
pub fn archive(
    slice: &RegistrySlice,
    stamp: &Stamp,
    reference: &str,
) -> Result<Decision<Workspace>, WgError> {
    stamp.require(&[Permission::WorkspaceAdmin])?;
    let found = resolve::workspace(slice, reference)?;
    let row = Workspace {
        archived_at: Some(stamp.at),
        updated_at: stamp.at,
        ..found.clone()
    };
    let ([event_id], _) = stamp.ids().take::<1>();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(row.id),
        row.id,
        EventPayload::WorkspaceArchived(WorkspaceArchived {}),
    );
    Ok(Decision::new(row.clone())
        .write(Write::update(row))
        .event(event))
}

/// `WorkspaceService.update_settings`: the update's keys overwrite the stored settings. A merge
/// that changes nothing leaves the row (and its `updated_at`) alone but still records the event.
pub fn update_settings(
    slice: &RegistrySlice,
    stamp: &Stamp,
    reference: &str,
    update: &JsonObject,
) -> Result<Decision<Workspace>, WgError> {
    stamp.require(&[Permission::WorkspaceAdmin])?;
    settings::validate_update(update)?;
    let found = resolve::workspace(slice, reference)?;
    let merged = settings::merge(&found.settings, update);
    let changed = merged != found.settings;
    let row = Workspace {
        settings: merged.clone(),
        updated_at: if changed { stamp.at } else { found.updated_at },
        ..found.clone()
    };
    let ([event_id], _) = stamp.ids().take::<1>();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(row.id),
        row.id,
        EventPayload::WorkspaceSettingsChanged(WorkspaceSettingsChanged {
            before: found.settings.clone(),
            after: merged,
        }),
    );
    let decision = Decision::new(row.clone()).event(event);
    Ok(if changed {
        decision.write(Write::update(row))
    } else {
        decision
    })
}
