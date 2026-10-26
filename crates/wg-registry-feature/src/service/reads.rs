//! The registry's `Session` readers: the lookups by slug, name, fingerprint, remote URL and checkout
//! path, the dependency reads, the counts of the workspace and project summaries, and the registry
//! slice that reference resolution runs in. Each one is a free function over `&mut Session`,
//! because an inherent impl cannot be written in this crate; the shared readers it builds on
//! (workspaces, projects, repositories, checkouts, refs) are `Session` methods of `wg-store`.

use wg_model::RegistrySlice;
use wg_model::entity::task::Task;
use wg_model::entity::{
    Project, Repository, RepositoryCheckout, RepositoryDependency, RepositoryRemote, Workspace,
};
use wg_model::{Row, Write};
use wg_store::{Session, many, one};
use wg_storecodec::Statement;
use wg_types::{Id, WgError};

use crate::spec::sql;

/// The workspace whose slug is `slug`, if any.
pub fn workspace_by_slug(
    session: &mut Session<'_>,
    slug: &str,
) -> Result<Option<Workspace>, WgError> {
    one(session, &sql::workspace_by_slug(slug))
}

/// The project in `workspace_id` whose slug is `slug`, if any.
pub fn project_by_slug(
    session: &mut Session<'_>,
    workspace_id: Id,
    slug: &str,
) -> Result<Option<Project>, WgError> {
    one(session, &sql::project_by_slug(workspace_id, slug))
}

/// The repository in `workspace_id` named `name`, if any.
pub fn repository_by_name(
    session: &mut Session<'_>,
    workspace_id: Id,
    name: &str,
) -> Result<Option<Repository>, WgError> {
    one(session, &sql::repository_by_name(workspace_id, name))
}

/// The repositories of `workspace_id` with identity fingerprint `fingerprint`, by name.
pub fn repository_by_fingerprint(
    session: &mut Session<'_>,
    workspace_id: Id,
    fingerprint: &str,
) -> Result<Vec<Repository>, WgError> {
    many(
        session,
        &sql::repository_by_fingerprint(workspace_id, fingerprint),
    )
}

/// The repositories of `workspace_id` with a remote whose normalised URL is in `urls`, by name.
pub fn repository_by_remote_urls(
    session: &mut Session<'_>,
    workspace_id: Id,
    urls: &[String],
) -> Result<Vec<Repository>, WgError> {
    many(session, &sql::repository_by_remote_urls(workspace_id, urls))
}

/// The repositories of `workspace_id` with a checkout at `path`, by name.
pub fn repository_by_checkout_path(
    session: &mut Session<'_>,
    workspace_id: Id,
    path: &str,
) -> Result<Vec<Repository>, WgError> {
    many(
        session,
        &sql::repository_by_checkout_path(workspace_id, path),
    )
}

/// The checkouts of `repository_id` in the order of the Python relationship: by creation time,
/// then by path.
pub fn checkouts_by_creation(
    session: &mut Session<'_>,
    repository_id: Id,
) -> Result<Vec<RepositoryCheckout>, WgError> {
    let mut found = session.checkouts(repository_id)?;
    found.sort_by_key(|checkout| checkout.created_at);
    Ok(found)
}

/// The remotes of `repository_id`, by name.
pub fn remotes(
    session: &mut Session<'_>,
    repository_id: Id,
) -> Result<Vec<RepositoryRemote>, WgError> {
    many(session, &sql::remotes(repository_id))
}

/// The dependencies that start at `repository_id`, by the repository they point at.
pub fn dependencies_from(
    session: &mut Session<'_>,
    repository_id: Id,
) -> Result<Vec<RepositoryDependency>, WgError> {
    many(session, &sql::dependencies_from(repository_id))
}

/// The dependencies that point at `repository_id`, by the repository they start from.
pub fn dependencies_to(
    session: &mut Session<'_>,
    repository_id: Id,
) -> Result<Vec<RepositoryDependency>, WgError> {
    many(session, &sql::dependencies_to(repository_id))
}

/// The dependency from `from` to `to`, if one is recorded.
pub fn dependency_between(
    session: &mut Session<'_>,
    from: Id,
    to: Id,
) -> Result<Option<RepositoryDependency>, WgError> {
    one(session, &sql::dependency_between(from, to))
}

/// The rows references in `workspace_id` resolve against: the workspace, its projects and their
/// memberships, and its repositories, archived ones included.
pub fn registry_slice(
    session: &mut Session<'_>,
    workspace_id: Id,
) -> Result<RegistrySlice, WgError> {
    let workspace = session.workspace_by_id(workspace_id)?.ok_or_else(|| {
        WgError::new(
            wg_types::ErrorCode::NotFound,
            format!("workspace {workspace_id} not found"),
        )
    })?;
    let mut writes = vec![Write::insert(Row::Workspace(workspace))];
    for project in session.projects(workspace_id, true)? {
        for link in session.project_repositories(project.id)? {
            writes.push(Write::insert(Row::ProjectRepository(link)));
        }
        writes.push(Write::insert(Row::Project(project)));
    }
    for repository in session.repositories(workspace_id, None, true)? {
        writes.push(Write::insert(Row::Repository(repository)));
    }
    Ok(RegistrySlice::default().apply_all(&writes))
}

/// The live tasks of `workspace_id` counted by status: `(status, count)` in status order.
pub fn workspace_task_counts(
    session: &mut Session<'_>,
    workspace_id: Id,
) -> Result<Vec<(String, i64)>, WgError> {
    pairs(session, &sql::workspace_task_counts(workspace_id))
}

/// The live tasks of `project_id` counted by status: `(status, count)` in status order.
pub fn project_task_counts(
    session: &mut Session<'_>,
    project_id: Id,
) -> Result<Vec<(String, i64)>, WgError> {
    pairs(session, &sql::project_task_counts(project_id))
}

/// The `limit` most recently updated live tasks of `project_id`, newest first.
pub fn project_recent_tasks(
    session: &mut Session<'_>,
    project_id: Id,
    limit: i64,
) -> Result<Vec<Task>, WgError> {
    many(session, &sql::project_recent_tasks(project_id, limit))
}

/// The attempts of `workspace_id` that are preparing, running or verifying.
pub fn workspace_active_attempts(
    session: &mut Session<'_>,
    workspace_id: Id,
) -> Result<i64, WgError> {
    let rows = session.rows(&sql::workspace_active_attempts(workspace_id))?;
    wg_storecodec::repo_sql::scalar_from_rows(&rows)
}

/// The jobs of `workspace_id` counted by status: `(status, count)` in status order.
pub fn workspace_job_counts(
    session: &mut Session<'_>,
    workspace_id: Id,
) -> Result<Vec<(String, i64)>, WgError> {
    pairs(session, &sql::workspace_job_counts(workspace_id))
}

/// Runs `statement` and decodes its `(text, integer)` rows.
fn pairs(session: &mut Session<'_>, statement: &Statement) -> Result<Vec<(String, i64)>, WgError> {
    wg_storecodec::repo_sql::pairs_from_rows(&session.rows(statement)?)
}
