//! Row-level writes: how a decision names the rows it inserts, replaces and deletes.
//!
//! Every table has one variant in each of three enums, all named after the entity struct:
//!
//! * [`Table`]: the SQL table name;
//! * [`Row`]: a whole row value (the entity struct);
//! * [`RowKey`]: the row's primary key, a bare [`Id`] for surrogate keys and named fields in
//!   primary-key column order for composite keys.
//!
//! A [`Write`] is an insert of a [`Row`], a whole-row replacement of a [`Row`] (optionally
//! guarded by the version it expects the stored row to have), or a delete by [`RowKey`]. The store
//! applies writes in order inside the decision's transaction; a guarded update that matches no row
//! fails the transaction with `version_conflict`.

use wg_types::Id;

use crate::entity::{
    AgentAttempt, Artifact, ChangeSet, ChangeSetItem, ExternalItem, Job, JobResourceClaim, JobRun,
    ManagedWorktree, Project, ProjectRepository, Repository, RepositoryCheckout,
    RepositoryDependency, RepositoryRef, RepositoryRemote, ResourceLease, RevisionManifest, Stack,
    StackItem, StackRepository, SystemCursor, Task, TaskComment, TaskDependency, TaskFile,
    TaskLink, TaskProject, TaskRelation, TaskRepository, TaskTag, Worker, Workspace,
};

/// A table the model writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Table {
    Workspace,
    Project,
    ProjectRepository,
    Repository,
    RepositoryCheckout,
    RepositoryRemote,
    RepositoryRef,
    RepositoryDependency,
    Task,
    TaskFile,
    TaskTag,
    TaskLink,
    TaskComment,
    TaskDependency,
    TaskRelation,
    TaskProject,
    TaskRepository,
    ExternalItem,
    AgentAttempt,
    RevisionManifest,
    ManagedWorktree,
    ResourceLease,
    ChangeSet,
    ChangeSetItem,
    Artifact,
    Stack,
    StackRepository,
    StackItem,
    Job,
    JobRun,
    JobResourceClaim,
    Worker,
    SystemCursor,
}

impl Table {
    /// Every table, in declaration order.
    pub const ALL: &'static [Table] = &[
        Table::Workspace,
        Table::Project,
        Table::ProjectRepository,
        Table::Repository,
        Table::RepositoryCheckout,
        Table::RepositoryRemote,
        Table::RepositoryRef,
        Table::RepositoryDependency,
        Table::Task,
        Table::TaskFile,
        Table::TaskTag,
        Table::TaskLink,
        Table::TaskComment,
        Table::TaskDependency,
        Table::TaskRelation,
        Table::TaskProject,
        Table::TaskRepository,
        Table::ExternalItem,
        Table::AgentAttempt,
        Table::RevisionManifest,
        Table::ManagedWorktree,
        Table::ResourceLease,
        Table::ChangeSet,
        Table::ChangeSetItem,
        Table::Artifact,
        Table::Stack,
        Table::StackRepository,
        Table::StackItem,
        Table::Job,
        Table::JobRun,
        Table::JobResourceClaim,
        Table::Worker,
        Table::SystemCursor,
    ];

    /// The SQL table name.
    pub const fn name(self) -> &'static str {
        match self {
            Table::Workspace => "workspaces",
            Table::Project => "projects",
            Table::ProjectRepository => "project_repositories",
            Table::Repository => "repositories",
            Table::RepositoryCheckout => "repository_checkouts",
            Table::RepositoryRemote => "repository_remotes",
            Table::RepositoryRef => "repository_refs",
            Table::RepositoryDependency => "repository_dependencies",
            Table::Task => "tasks",
            Table::TaskFile => "task_files",
            Table::TaskTag => "task_tags",
            Table::TaskLink => "task_links",
            Table::TaskComment => "task_comments",
            Table::TaskDependency => "task_dependencies",
            Table::TaskRelation => "task_relations",
            Table::TaskProject => "task_projects",
            Table::TaskRepository => "task_repositories",
            Table::ExternalItem => "external_items",
            Table::AgentAttempt => "agent_attempts",
            Table::RevisionManifest => "revision_manifests",
            Table::ManagedWorktree => "managed_worktrees",
            Table::ResourceLease => "resource_leases",
            Table::ChangeSet => "change_sets",
            Table::ChangeSetItem => "change_set_items",
            Table::Artifact => "artifacts",
            Table::Stack => "stacks",
            Table::StackRepository => "stack_repositories",
            Table::StackItem => "stack_items",
            Table::Job => "jobs",
            Table::JobRun => "job_runs",
            Table::JobResourceClaim => "job_resource_claims",
            Table::Worker => "workers",
            Table::SystemCursor => "system_cursors",
        }
    }
}

/// A whole row of one table.
///
/// Rows are held by value: the vectors that carry them (`Decision::writes`) keep every element
/// behind a shared pointer already, so boxing the large variants would only add an allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Workspace(Workspace),
    Project(Project),
    ProjectRepository(ProjectRepository),
    Repository(Repository),
    RepositoryCheckout(RepositoryCheckout),
    RepositoryRemote(RepositoryRemote),
    RepositoryRef(RepositoryRef),
    RepositoryDependency(RepositoryDependency),
    Task(Task),
    TaskFile(TaskFile),
    TaskTag(TaskTag),
    TaskLink(TaskLink),
    TaskComment(TaskComment),
    TaskDependency(TaskDependency),
    TaskRelation(TaskRelation),
    TaskProject(TaskProject),
    TaskRepository(TaskRepository),
    ExternalItem(ExternalItem),
    AgentAttempt(AgentAttempt),
    RevisionManifest(RevisionManifest),
    ManagedWorktree(ManagedWorktree),
    ResourceLease(ResourceLease),
    ChangeSet(ChangeSet),
    ChangeSetItem(ChangeSetItem),
    Artifact(Artifact),
    Stack(Stack),
    StackRepository(StackRepository),
    StackItem(StackItem),
    Job(Job),
    JobRun(JobRun),
    JobResourceClaim(JobResourceClaim),
    Worker(Worker),
    SystemCursor(SystemCursor),
}

impl Row {
    /// The table the row belongs to.
    pub fn table(&self) -> Table {
        self.key().table()
    }

    /// The row's primary key.
    pub fn key(&self) -> RowKey {
        match self {
            Row::Workspace(row) => row.row_key(),
            Row::Project(row) => row.row_key(),
            Row::ProjectRepository(row) => row.row_key(),
            Row::Repository(row) => row.row_key(),
            Row::RepositoryCheckout(row) => row.row_key(),
            Row::RepositoryRemote(row) => row.row_key(),
            Row::RepositoryRef(row) => row.row_key(),
            Row::RepositoryDependency(row) => row.row_key(),
            Row::Task(row) => row.row_key(),
            Row::TaskFile(row) => row.row_key(),
            Row::TaskTag(row) => row.row_key(),
            Row::TaskLink(row) => row.row_key(),
            Row::TaskComment(row) => row.row_key(),
            Row::TaskDependency(row) => row.row_key(),
            Row::TaskRelation(row) => row.row_key(),
            Row::TaskProject(row) => row.row_key(),
            Row::TaskRepository(row) => row.row_key(),
            Row::ExternalItem(row) => row.row_key(),
            Row::AgentAttempt(row) => row.row_key(),
            Row::RevisionManifest(row) => row.row_key(),
            Row::ManagedWorktree(row) => row.row_key(),
            Row::ResourceLease(row) => row.row_key(),
            Row::ChangeSet(row) => row.row_key(),
            Row::ChangeSetItem(row) => row.row_key(),
            Row::Artifact(row) => row.row_key(),
            Row::Stack(row) => row.row_key(),
            Row::StackRepository(row) => row.row_key(),
            Row::StackItem(row) => row.row_key(),
            Row::Job(row) => row.row_key(),
            Row::JobRun(row) => row.row_key(),
            Row::JobResourceClaim(row) => row.row_key(),
            Row::Worker(row) => row.row_key(),
            Row::SystemCursor(row) => row.row_key(),
        }
    }
}

/// The primary key of one row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RowKey {
    Workspace(Id),
    Project(Id),
    ProjectRepository { project_id: Id, repository_id: Id },
    Repository(Id),
    RepositoryCheckout(Id),
    RepositoryRemote(Id),
    RepositoryRef(Id),
    RepositoryDependency(Id),
    Task(Id),
    TaskFile { task_id: Id, position: i64 },
    TaskTag { task_id: Id, tag: String },
    TaskLink(Id),
    TaskComment(Id),
    TaskDependency(Id),
    TaskRelation(Id),
    TaskProject { task_id: Id, project_id: Id },
    TaskRepository { task_id: Id, repository_id: Id },
    ExternalItem(Id),
    AgentAttempt(Id),
    RevisionManifest(Id),
    ManagedWorktree(Id),
    ResourceLease(String),
    ChangeSet(Id),
    ChangeSetItem(Id),
    Artifact(Id),
    Stack(Id),
    StackRepository { stack_id: Id, repository_id: Id },
    StackItem(Id),
    Job(Id),
    JobRun(Id),
    JobResourceClaim { job_id: Id, resource_key: String },
    Worker(String),
    SystemCursor(String),
}

impl RowKey {
    /// The table the key belongs to.
    pub fn table(&self) -> Table {
        match self {
            RowKey::Workspace(_) => Table::Workspace,
            RowKey::Project(_) => Table::Project,
            RowKey::ProjectRepository { .. } => Table::ProjectRepository,
            RowKey::Repository(_) => Table::Repository,
            RowKey::RepositoryCheckout(_) => Table::RepositoryCheckout,
            RowKey::RepositoryRemote(_) => Table::RepositoryRemote,
            RowKey::RepositoryRef(_) => Table::RepositoryRef,
            RowKey::RepositoryDependency(_) => Table::RepositoryDependency,
            RowKey::Task(_) => Table::Task,
            RowKey::TaskFile { .. } => Table::TaskFile,
            RowKey::TaskTag { .. } => Table::TaskTag,
            RowKey::TaskLink(_) => Table::TaskLink,
            RowKey::TaskComment(_) => Table::TaskComment,
            RowKey::TaskDependency(_) => Table::TaskDependency,
            RowKey::TaskRelation(_) => Table::TaskRelation,
            RowKey::TaskProject { .. } => Table::TaskProject,
            RowKey::TaskRepository { .. } => Table::TaskRepository,
            RowKey::ExternalItem(_) => Table::ExternalItem,
            RowKey::AgentAttempt(_) => Table::AgentAttempt,
            RowKey::RevisionManifest(_) => Table::RevisionManifest,
            RowKey::ManagedWorktree(_) => Table::ManagedWorktree,
            RowKey::ResourceLease(_) => Table::ResourceLease,
            RowKey::ChangeSet(_) => Table::ChangeSet,
            RowKey::ChangeSetItem(_) => Table::ChangeSetItem,
            RowKey::Artifact(_) => Table::Artifact,
            RowKey::Stack(_) => Table::Stack,
            RowKey::StackRepository { .. } => Table::StackRepository,
            RowKey::StackItem(_) => Table::StackItem,
            RowKey::Job(_) => Table::Job,
            RowKey::JobRun(_) => Table::JobRun,
            RowKey::JobResourceClaim { .. } => Table::JobResourceClaim,
            RowKey::Worker(_) => Table::Worker,
            RowKey::SystemCursor(_) => Table::SystemCursor,
        }
    }
}

/// One row-level change, applied by the store in decision order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// Inserts a new row; a primary-key or unique clash fails the transaction.
    Insert(Row),
    /// Replaces every column of the row with this primary key. With `expected_version`, the store
    /// updates only when the stored `version` column equals it (`tasks` is the only versioned
    /// table here); the new value carries the incremented version.
    Update {
        row: Row,
        expected_version: Option<i64>,
    },
    /// Deletes the row with this primary key; cascades are the database's.
    Delete(RowKey),
}

impl Write {
    pub fn insert(row: impl Into<Row>) -> Self {
        Write::Insert(row.into())
    }

    pub fn update(row: impl Into<Row>) -> Self {
        Write::Update {
            row: row.into(),
            expected_version: None,
        }
    }

    /// A guarded update: only applies when the stored version equals `expected_version`.
    pub fn update_versioned(row: impl Into<Row>, expected_version: i64) -> Self {
        Write::Update {
            row: row.into(),
            expected_version: Some(expected_version),
        }
    }

    pub fn delete(key: RowKey) -> Self {
        Write::Delete(key)
    }

    /// The table the write touches.
    pub fn table(&self) -> Table {
        match self {
            Write::Insert(row) | Write::Update { row, .. } => row.table(),
            Write::Delete(key) => key.table(),
        }
    }

    /// The primary key of the row the write touches.
    pub fn key(&self) -> RowKey {
        match self {
            Write::Insert(row) | Write::Update { row, .. } => row.key(),
            Write::Delete(key) => key.clone(),
        }
    }
}
