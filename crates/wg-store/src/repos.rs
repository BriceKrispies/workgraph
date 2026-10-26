//! Reads for the workspace, project and repository family (SH-05): workspaces, projects,
//! repositories, their checkouts, remotes, refs and dependencies.
//!
//! The statements are built by `wg_storecodec::repo_sql` as values; this module only runs them
//! through the session and decodes the rows with the table codecs. Writes go through the generic
//! [`Session::apply_write`] and [`Session::apply_decision`] like every other family.

use wg_model::entity::{
    Project, ProjectRepository, Repository, RepositoryCheckout, RepositoryDependency,
    RepositoryRef, RepositoryRemote, Workspace,
};
use wg_storecodec::repo_sql;
use wg_types::{Id, WgError};

use crate::Session;
use crate::query::{many, one};

impl Session<'_> {
    /// The workspace whose slug is `slug`, if any.
    pub fn workspace_by_slug(&mut self, slug: &str) -> Result<Option<Workspace>, WgError> {
        one(self, &repo_sql::workspace_by_slug(slug))
    }

    /// Every workspace, ordered by slug.
    pub fn workspaces(&mut self) -> Result<Vec<Workspace>, WgError> {
        many(self, &repo_sql::workspaces())
    }

    /// The project in `workspace_id` whose slug is `slug`, if any.
    pub fn project_by_slug(
        &mut self,
        workspace_id: Id,
        slug: &str,
    ) -> Result<Option<Project>, WgError> {
        one(self, &repo_sql::project_by_slug(workspace_id, slug))
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

    /// The repository in `workspace_id` named `name`, if any.
    pub fn repository_by_name(
        &mut self,
        workspace_id: Id,
        name: &str,
    ) -> Result<Option<Repository>, WgError> {
        one(self, &repo_sql::repository_by_name(workspace_id, name))
    }

    /// The repositories of `workspace_id` with identity fingerprint `fingerprint`, by name.
    pub fn repository_by_fingerprint(
        &mut self,
        workspace_id: Id,
        fingerprint: &str,
    ) -> Result<Vec<Repository>, WgError> {
        many(
            self,
            &repo_sql::repository_by_fingerprint(workspace_id, fingerprint),
        )
    }

    /// The repositories of `workspace_id` with a remote whose normalised URL is in `urls`, by name.
    pub fn repository_by_remote_urls(
        &mut self,
        workspace_id: Id,
        urls: &[String],
    ) -> Result<Vec<Repository>, WgError> {
        many(
            self,
            &repo_sql::repository_by_remote_urls(workspace_id, urls),
        )
    }

    /// The repositories of `workspace_id` with a checkout at `path`, by name.
    pub fn repository_by_checkout_path(
        &mut self,
        workspace_id: Id,
        path: &str,
    ) -> Result<Vec<Repository>, WgError> {
        many(
            self,
            &repo_sql::repository_by_checkout_path(workspace_id, path),
        )
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

    /// The remotes of `repository_id`, by name.
    pub fn remotes(&mut self, repository_id: Id) -> Result<Vec<RepositoryRemote>, WgError> {
        many(self, &repo_sql::remotes(repository_id))
    }

    /// The refs of `repository_id`, by name.
    pub fn refs(&mut self, repository_id: Id) -> Result<Vec<RepositoryRef>, WgError> {
        many(self, &repo_sql::refs(repository_id))
    }

    /// The dependencies that start at `repository_id`, by the repository they point at.
    pub fn dependencies_from(
        &mut self,
        repository_id: Id,
    ) -> Result<Vec<RepositoryDependency>, WgError> {
        many(self, &repo_sql::dependencies_from(repository_id))
    }

    /// The dependencies that point at `repository_id`, by the repository they start from.
    pub fn dependencies_to(
        &mut self,
        repository_id: Id,
    ) -> Result<Vec<RepositoryDependency>, WgError> {
        many(self, &repo_sql::dependencies_to(repository_id))
    }

    /// The dependency from `from` to `to`, if one is recorded.
    pub fn dependency_between(
        &mut self,
        from: Id,
        to: Id,
    ) -> Result<Option<RepositoryDependency>, WgError> {
        one(self, &repo_sql::dependency_between(from, to))
    }
}
