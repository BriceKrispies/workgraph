//! [`RegistrySlice`]: workspaces, projects and repositories.

use rpds::RedBlackTreeMapSync;
use wg_types::Id;

use crate::entity::{
    Project, ProjectRepository, Repository, RepositoryCheckout, RepositoryDependency,
    RepositoryRef, RepositoryRemote, Workspace,
};
use crate::slice::children;
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the workspace, project and repository families.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegistrySlice {
    pub workspaces: RedBlackTreeMapSync<Id, Workspace>,
    pub projects: RedBlackTreeMapSync<Id, Project>,
    /// Keyed `(project_id, repository_id)`.
    pub project_repositories: RedBlackTreeMapSync<(Id, Id), ProjectRepository>,
    pub repositories: RedBlackTreeMapSync<Id, Repository>,
    pub checkouts: RedBlackTreeMapSync<Id, RepositoryCheckout>,
    pub remotes: RedBlackTreeMapSync<Id, RepositoryRemote>,
    pub refs: RedBlackTreeMapSync<Id, RepositoryRef>,
    pub repository_dependencies: RedBlackTreeMapSync<Id, RepositoryDependency>,
}

impl RegistrySlice {
    pub fn workspace(&self, id: Id) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    pub fn workspace_by_slug(&self, slug: &str) -> Option<&Workspace> {
        self.workspaces.values().find(|ws| ws.slug == slug)
    }

    pub fn project(&self, id: Id) -> Option<&Project> {
        self.projects.get(&id)
    }

    pub fn repository(&self, id: Id) -> Option<&Repository> {
        self.repositories.get(&id)
    }

    /// The memberships of a project, by repository id.
    pub fn project_repositories_of(
        &self,
        project_id: Id,
    ) -> impl Iterator<Item = &ProjectRepository> {
        children(&self.project_repositories, project_id)
    }

    /// The checkouts of a repository, by checkout id.
    pub fn checkouts_of(&self, repository_id: Id) -> impl Iterator<Item = &RepositoryCheckout> {
        self.checkouts
            .values()
            .filter(move |row| row.repository_id == repository_id)
    }

    /// The remotes of a repository, by remote id.
    pub fn remotes_of(&self, repository_id: Id) -> impl Iterator<Item = &RepositoryRemote> {
        self.remotes
            .values()
            .filter(move |row| row.repository_id == repository_id)
    }

    /// The observed refs of a repository, by ref id.
    pub fn refs_of(&self, repository_id: Id) -> impl Iterator<Item = &RepositoryRef> {
        self.refs
            .values()
            .filter(move |row| row.repository_id == repository_id)
    }

    /// The declared dependencies from a repository, by dependency id.
    pub fn dependencies_of(
        &self,
        repository_id: Id,
    ) -> impl Iterator<Item = &RepositoryDependency> {
        self.repository_dependencies
            .values()
            .filter(move |row| row.from_repository_id == repository_id)
    }

    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => self.put(row),
            Write::Delete(key) => self.remove(key),
        }
    }

    /// The slice after every write, in order.
    #[must_use]
    pub fn apply_all<'a>(&self, writes: impl IntoIterator<Item = &'a Write>) -> Self {
        writes
            .into_iter()
            .fold(self.clone(), |slice, write| slice.apply(write))
    }

    fn put(&self, row: &Row) -> Self {
        match row {
            Row::Workspace(row) => RegistrySlice {
                workspaces: self.workspaces.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::Project(row) => RegistrySlice {
                projects: self.projects.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::ProjectRepository(row) => RegistrySlice {
                project_repositories: self
                    .project_repositories
                    .insert((row.project_id, row.repository_id), row.clone()),
                ..self.clone()
            },
            Row::Repository(row) => RegistrySlice {
                repositories: self.repositories.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::RepositoryCheckout(row) => RegistrySlice {
                checkouts: self.checkouts.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::RepositoryRemote(row) => RegistrySlice {
                remotes: self.remotes.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::RepositoryRef(row) => RegistrySlice {
                refs: self.refs.insert(row.id, row.clone()),
                ..self.clone()
            },
            Row::RepositoryDependency(row) => RegistrySlice {
                repository_dependencies: self.repository_dependencies.insert(row.id, row.clone()),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }

    fn remove(&self, key: &RowKey) -> Self {
        match key {
            RowKey::Workspace(id) => RegistrySlice {
                workspaces: self.workspaces.remove(id),
                ..self.clone()
            },
            RowKey::Project(id) => RegistrySlice {
                projects: self.projects.remove(id),
                ..self.clone()
            },
            RowKey::ProjectRepository {
                project_id,
                repository_id,
            } => RegistrySlice {
                project_repositories: self
                    .project_repositories
                    .remove(&(*project_id, *repository_id)),
                ..self.clone()
            },
            RowKey::Repository(id) => RegistrySlice {
                repositories: self.repositories.remove(id),
                ..self.clone()
            },
            RowKey::RepositoryCheckout(id) => RegistrySlice {
                checkouts: self.checkouts.remove(id),
                ..self.clone()
            },
            RowKey::RepositoryRemote(id) => RegistrySlice {
                remotes: self.remotes.remove(id),
                ..self.clone()
            },
            RowKey::RepositoryRef(id) => RegistrySlice {
                refs: self.refs.remove(id),
                ..self.clone()
            },
            RowKey::RepositoryDependency(id) => RegistrySlice {
                repository_dependencies: self.repository_dependencies.remove(id),
                ..self.clone()
            },
            _ => self.clone(),
        }
    }
}
