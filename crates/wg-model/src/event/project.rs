//! Payloads of the `project.*` events (`core/services/projects.py`, the tracker sync).

use rpds::VectorSync;
use serde::Serialize;
use wg_types::Id;

use crate::event::Changes;

/// `project.created`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectCreated {
    pub slug: String,
    pub name: String,
    pub repository_ids: VectorSync<Id>,
}

/// `project.updated`: the changed fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectUpdated {
    pub changes: Changes,
}

/// `project.repository_added`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectRepositoryAdded {
    pub repository_id: Id,
    pub role: String,
}

/// `project.repository_removed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectRepositoryRemoved {
    pub repository_id: Id,
}
