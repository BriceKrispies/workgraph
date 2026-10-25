//! The `project` family: `projects` and `project_repositories`.

use wg_types::{Id, Timestamp};

use crate::entity::JsonObject;
use crate::write::{Row, RowKey};

/// A row of `projects`. `status` is `active` or `archived`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: Id,
    pub workspace_id: Id,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub status: String,
    pub settings: JsonObject,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Project {
    pub fn row_key(&self) -> RowKey {
        RowKey::Project(self.id)
    }
}

/// A row of `project_repositories`: primary key `(project_id, repository_id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRepository {
    pub project_id: Id,
    pub repository_id: Id,
    /// `member` unless the caller named another role.
    pub role: String,
    pub access_mode: Option<String>,
    pub created_at: Timestamp,
}

impl ProjectRepository {
    pub fn row_key(&self) -> RowKey {
        RowKey::ProjectRepository {
            project_id: self.project_id,
            repository_id: self.repository_id,
        }
    }
}

impl From<Project> for Row {
    fn from(row: Project) -> Self {
        Row::Project(row)
    }
}

impl From<ProjectRepository> for Row {
    fn from(row: ProjectRepository) -> Self {
        Row::ProjectRepository(row)
    }
}
