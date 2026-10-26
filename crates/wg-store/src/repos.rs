//! Reads for the workspace, project and repository family (SH-05) that other features share:
//! workspaces, projects, repositories, their checkouts and refs, and the registry slice the
//! runtime's git and stack code resolve references in.
//!
//! The statements are built by `wg_storecodec::repo_sql` as values; this module only runs them
//! through the session and decodes the rows with the table codecs. The registry-only lookups and
//! counts are free functions in `wg-registry-feature`'s `service::reads`. Writes go through the
//! generic [`Session::apply_write`] and [`Session::apply_decision`] like every other family.

use wg_model::entity::{
    Project, ProjectRepository, Repository, RepositoryCheckout, RepositoryRef, Workspace,
};
use wg_model::{RegistrySlice, Row, RowKey, Write};
use wg_storecodec::repo_sql;
use wg_types::{Id, WgError};

use crate::Session;
use crate::query::many;

impl Session<'_> {
    /// Every workspace, ordered by slug.
    pub fn workspaces(&mut self) -> Result<Vec<Workspace>, WgError> {
        many(self, &repo_sql::workspaces())
    }

    /// The projects of `workspace_id`, ordered by slug; only active ones unless `include_archived`.
    pub fn projects(
        &mut self,
        workspace_id: Id,
        include_archived: bool,
    ) -> Result<Vec<Project>, WgError> {
        many(self, &repo_sql::projects(workspace_id, include_archived))
    }

    /// The repository links of `project_id`, ordered by repository id.
    pub fn project_repositories(
        &mut self,
        project_id: Id,
    ) -> Result<Vec<ProjectRepository>, WgError> {
        many(self, &repo_sql::project_repositories(project_id))
    }

    /// The repositories of `workspace_id`, by name; `project_id` narrows to one project's members,
    /// and archived repositories are left out unless `include_archived`.
    pub fn repositories(
        &mut self,
        workspace_id: Id,
        project_id: Option<Id>,
        include_archived: bool,
    ) -> Result<Vec<Repository>, WgError> {
        many(
            self,
            &repo_sql::repositories(workspace_id, project_id, include_archived),
        )
    }

    /// The checkouts of `repository_id`, by path.
    pub fn checkouts(&mut self, repository_id: Id) -> Result<Vec<RepositoryCheckout>, WgError> {
        many(self, &repo_sql::checkouts(repository_id))
    }

    /// The refs of `repository_id`, by name.
    pub fn refs(&mut self, repository_id: Id) -> Result<Vec<RepositoryRef>, WgError> {
        many(self, &repo_sql::refs(repository_id))
    }

    /// The registry slice a decision on workspaces and projects reads: every workspace and, with
    /// `workspace_id`, that workspace's projects with their memberships and its repositories
    /// (archived ones included, as reference resolution needs them).
    pub fn registry_world(&mut self, workspace_id: Option<Id>) -> Result<RegistrySlice, WgError> {
        let workspaces = self.workspaces()?;
        let scoped = match workspace_id {
            Some(scope) => {
                let mut rows: Vec<Row> = Vec::new();
                for project in self.projects(scope, true)? {
                    rows.extend(
                        self.project_repositories(project.id)?
                            .into_iter()
                            .map(Row::from),
                    );
                    rows.push(Row::Project(project));
                }
                rows.extend(
                    self.repositories(scope, None, true)?
                        .into_iter()
                        .map(Row::from),
                );
                rows
            }
            None => Vec::new(),
        };
        let writes: Vec<Write> = workspaces
            .into_iter()
            .map(Row::from)
            .chain(scoped)
            .map(Write::Insert)
            .collect();
        Ok(RegistrySlice::default().apply_all(&writes))
    }

    /// The workspace row whose id is `workspace_id`, if any.
    pub fn workspace_by_id(&mut self, workspace_id: Id) -> Result<Option<Workspace>, WgError> {
        self.fetch(&RowKey::Workspace(workspace_id))
    }
}
