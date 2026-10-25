//! Payloads of the `workspace.*` events (`core/services/workspaces.py`).

use serde::Serialize;

use crate::entity::JsonObject;

/// `workspace.created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceCreated {
    pub slug: String,
    pub name: String,
    pub key_prefix: String,
}

/// `workspace.settings_changed`: the whole settings object before and after the merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceSettingsChanged {
    pub before: JsonObject,
    pub after: JsonObject,
}

/// `workspace.archived`: no data (`{}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct WorkspaceArchived {}
