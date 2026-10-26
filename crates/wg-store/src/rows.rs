//! Row codecs: one [`TableRow`] implementation per model table.
//!
//! `COLUMNS` lists the table's columns in the order of `contracts/schema-0011.sql`; `values`
//! returns the bound parameters in that order and `decode` reads a row back by column name. The
//! model's entity structs are the rows (their fields are the columns), so the codecs add no
//! intermediate type.

use rusqlite::Row as SqlRow;
use rusqlite::types::Value as Sql;
use wg_model::entity::{
    ExternalItem, Project, ProjectRepository, Repository, RepositoryCheckout, RepositoryDependency,
    RepositoryRef, RepositoryRemote, Task, TaskComment, TaskDependency, TaskFile, TaskLink,
    TaskProject, TaskRelation, TaskRepository, TaskTag, Workspace,
};
use wg_model::{Row, RowKey, Table};
use wg_types::{TaskKey, WgError};

use crate::codec::{
    Cols, bool_sql, id_sql, object_sql, opt_id_sql, opt_int_sql, opt_object_sql, opt_text_sql,
    opt_ts_sql, strings_sql, text_sql, ts_sql,
};

/// A model value that is one row of one table.
pub trait TableRow: Sized {
    const TABLE: Table;

    /// The bound parameters, in the order of [`columns`].
    fn values(&self) -> Result<Vec<Sql>, WgError>;

    /// Reads the row from a result that selects at least [`columns`].
    fn decode(cols: &Cols<'_, '_>) -> rusqlite::Result<Self>;

    /// `decode` over a raw rusqlite row, as `query_map` and `query_row` want it.
    fn from_sql_row(row: &SqlRow<'_>) -> rusqlite::Result<Self> {
        Self::decode(&Cols::new(row))
    }
}

/// The columns of `table`, in schema order.
pub const fn columns(table: Table) -> &'static [&'static str] {
    match table {
        Table::Workspace => &[
            "id",
            "slug",
            "name",
            "description",
            "key_prefix",
            "next_task_number",
            "settings",
            "archived_at",
            "created_at",
            "updated_at",
        ],
        Table::Project => &[
            "id",
            "workspace_id",
            "slug",
            "name",
            "description",
            "status",
            "settings",
            "created_at",
            "updated_at",
        ],
        Table::ProjectRepository => &[
            "project_id",
            "repository_id",
            "role",
            "access_mode",
            "created_at",
        ],
        Table::Repository => &[
            "id",
            "workspace_id",
            "name",
            "description",
            "identity_fingerprint",
            "default_branch",
            "tracked_branches",
            "access_mode",
            "health_status",
            "health_detail",
            "health_checked_at",
            "is_shallow",
            "last_scanned_at",
            "archived_at",
            "metadata",
            "created_at",
            "updated_at",
        ],
        Table::RepositoryCheckout => &[
            "id",
            "repository_id",
            "path",
            "kind",
            "is_primary",
            "access_mode",
            "available",
            "last_seen_at",
            "created_at",
            "updated_at",
        ],
        Table::RepositoryRemote => &[
            "id",
            "repository_id",
            "name",
            "url",
            "normalized_url",
            "created_at",
            "updated_at",
        ],
        Table::RepositoryRef => &["id", "repository_id", "name", "sha", "updated_at"],
        Table::RepositoryDependency => &[
            "id",
            "from_repository_id",
            "to_repository_id",
            "kind",
            "description",
            "created_at",
        ],
        Table::Task => &[
            "id",
            "workspace_id",
            "number",
            "key",
            "title",
            "description",
            "description_hash",
            "acceptance_criteria",
            "status",
            "priority",
            "category",
            "parent_id",
            "assignee",
            "assigned_agent",
            "claimed_by",
            "claim_expires_at",
            "auto_execute",
            "estimated_effort",
            "metadata",
            "due_at",
            "scheduled_at",
            "created_by",
            "version",
            "created_at",
            "updated_at",
            "started_at",
            "completed_at",
            "accepted_at",
            "archived_at",
            "current_classification_id",
        ],
        Table::TaskFile => &["task_id", "position", "path", "anchor", "note"],
        Table::TaskTag => &["task_id", "tag"],
        Table::TaskLink => &[
            "id",
            "task_id",
            "kind",
            "provider",
            "url",
            "normalized_url",
            "repo_full_name",
            "number",
            "repository_id",
            "sources",
            "title",
            "created_by",
            "created_at",
            "updated_at",
        ],
        Table::TaskComment => &[
            "id",
            "task_id",
            "attempt_id",
            "author",
            "kind",
            "body",
            "data",
            "created_at",
        ],
        Table::TaskDependency => &["id", "task_id", "depends_on_id", "created_by", "created_at"],
        Table::TaskRelation => &[
            "id",
            "source_task_id",
            "target_task_id",
            "relation_type",
            "created_by",
            "created_at",
        ],
        Table::TaskProject => &["task_id", "project_id"],
        Table::TaskRepository => &["task_id", "repository_id", "access", "created_at"],
        Table::ExternalItem => &[
            "id",
            "workspace_id",
            "task_id",
            "connection",
            "external_id",
            "external_key",
            "url",
            "tracker_status",
            "tracker_status_category",
            "tracker_resolution",
            "remote_updated_at",
            "snapshot",
            "snapshot_hash",
            "synced_at",
            "missing_since",
            "created_at",
            "updated_at",
        ],
    }
}

/// The primary-key columns of `table`, in key order (the order of [`key_values`]).
pub const fn primary_key(table: Table) -> &'static [&'static str] {
    match table {
        Table::ProjectRepository => &["project_id", "repository_id"],
        Table::TaskFile => &["task_id", "position"],
        Table::TaskTag => &["task_id", "tag"],
        Table::TaskProject => &["task_id", "project_id"],
        Table::TaskRepository => &["task_id", "repository_id"],
        _ => &["id"],
    }
}

/// The primary-key values of `key`, in the order of [`primary_key`].
pub fn key_values(key: &RowKey) -> Vec<Sql> {
    match key {
        RowKey::Workspace(id)
        | RowKey::Project(id)
        | RowKey::Repository(id)
        | RowKey::RepositoryCheckout(id)
        | RowKey::RepositoryRemote(id)
        | RowKey::RepositoryRef(id)
        | RowKey::RepositoryDependency(id)
        | RowKey::Task(id)
        | RowKey::TaskLink(id)
        | RowKey::TaskComment(id)
        | RowKey::TaskDependency(id)
        | RowKey::TaskRelation(id)
        | RowKey::ExternalItem(id) => vec![id_sql(*id)],
        RowKey::ProjectRepository {
            project_id,
            repository_id,
        } => vec![id_sql(*project_id), id_sql(*repository_id)],
        RowKey::TaskFile { task_id, position } => {
            vec![id_sql(*task_id), Sql::Integer(*position)]
        }
        RowKey::TaskTag { task_id, tag } => vec![id_sql(*task_id), text_sql(tag)],
        RowKey::TaskProject {
            task_id,
            project_id,
        } => vec![id_sql(*task_id), id_sql(*project_id)],
        RowKey::TaskRepository {
            task_id,
            repository_id,
        } => vec![id_sql(*task_id), id_sql(*repository_id)],
    }
}

/// The bound parameters of any row, in the order of [`columns`] for its table.
pub fn row_values(row: &Row) -> Result<Vec<Sql>, WgError> {
    match row {
        Row::Workspace(row) => row.values(),
        Row::Project(row) => row.values(),
        Row::ProjectRepository(row) => row.values(),
        Row::Repository(row) => row.values(),
        Row::RepositoryCheckout(row) => row.values(),
        Row::RepositoryRemote(row) => row.values(),
        Row::RepositoryRef(row) => row.values(),
        Row::RepositoryDependency(row) => row.values(),
        Row::Task(row) => row.values(),
        Row::TaskFile(row) => row.values(),
        Row::TaskTag(row) => row.values(),
        Row::TaskLink(row) => row.values(),
        Row::TaskComment(row) => row.values(),
        Row::TaskDependency(row) => row.values(),
        Row::TaskRelation(row) => row.values(),
        Row::TaskProject(row) => row.values(),
        Row::TaskRepository(row) => row.values(),
        Row::ExternalItem(row) => row.values(),
    }
}

impl TableRow for Workspace {
    const TABLE: Table = Table::Workspace;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            text_sql(&self.slug),
            text_sql(&self.name),
            text_sql(&self.description),
            text_sql(&self.key_prefix),
            Sql::Integer(self.next_task_number),
            object_sql(&self.settings)?,
            opt_ts_sql(self.archived_at),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(Workspace {
            id: c.id("id")?,
            slug: c.text("slug")?,
            name: c.text("name")?,
            description: c.text("description")?,
            key_prefix: c.text("key_prefix")?,
            next_task_number: c.int("next_task_number")?,
            settings: c.object("settings")?,
            archived_at: c.opt_ts("archived_at")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for Project {
    const TABLE: Table = Table::Project;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.slug),
            text_sql(&self.name),
            text_sql(&self.description),
            text_sql(&self.status),
            object_sql(&self.settings)?,
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(Project {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            slug: c.text("slug")?,
            name: c.text("name")?,
            description: c.text("description")?,
            status: c.text("status")?,
            settings: c.object("settings")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for ProjectRepository {
    const TABLE: Table = Table::ProjectRepository;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.project_id),
            id_sql(self.repository_id),
            text_sql(&self.role),
            opt_text_sql(&self.access_mode),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(ProjectRepository {
            project_id: c.id("project_id")?,
            repository_id: c.id("repository_id")?,
            role: c.text("role")?,
            access_mode: c.opt_text("access_mode")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for Repository {
    const TABLE: Table = Table::Repository;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.name),
            text_sql(&self.description),
            opt_text_sql(&self.identity_fingerprint),
            opt_text_sql(&self.default_branch),
            strings_sql(&self.tracked_branches)?,
            text_sql(self.access_mode.as_str()),
            text_sql(&self.health_status),
            object_sql(&self.health_detail)?,
            opt_ts_sql(self.health_checked_at),
            bool_sql(self.is_shallow),
            opt_ts_sql(self.last_scanned_at),
            opt_ts_sql(self.archived_at),
            object_sql(&self.metadata)?,
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(Repository {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            name: c.text("name")?,
            description: c.text("description")?,
            identity_fingerprint: c.opt_text("identity_fingerprint")?,
            default_branch: c.opt_text("default_branch")?,
            tracked_branches: c.strings("tracked_branches")?,
            access_mode: c.parsed("access_mode")?,
            health_status: c.text("health_status")?,
            health_detail: c.object("health_detail")?,
            health_checked_at: c.opt_ts("health_checked_at")?,
            is_shallow: c.flag("is_shallow")?,
            last_scanned_at: c.opt_ts("last_scanned_at")?,
            archived_at: c.opt_ts("archived_at")?,
            metadata: c.object("metadata")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for RepositoryCheckout {
    const TABLE: Table = Table::RepositoryCheckout;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.repository_id),
            text_sql(&self.path),
            text_sql(&self.kind),
            bool_sql(self.is_primary),
            opt_text_sql(&self.access_mode),
            bool_sql(self.available),
            opt_ts_sql(self.last_seen_at),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(RepositoryCheckout {
            id: c.id("id")?,
            repository_id: c.id("repository_id")?,
            path: c.text("path")?,
            kind: c.text("kind")?,
            is_primary: c.flag("is_primary")?,
            access_mode: c.opt_text("access_mode")?,
            available: c.flag("available")?,
            last_seen_at: c.opt_ts("last_seen_at")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for RepositoryRemote {
    const TABLE: Table = Table::RepositoryRemote;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.repository_id),
            text_sql(&self.name),
            text_sql(&self.url),
            text_sql(&self.normalized_url),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(RepositoryRemote {
            id: c.id("id")?,
            repository_id: c.id("repository_id")?,
            name: c.text("name")?,
            url: c.text("url")?,
            normalized_url: c.text("normalized_url")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for RepositoryRef {
    const TABLE: Table = Table::RepositoryRef;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.repository_id),
            text_sql(&self.name),
            text_sql(&self.sha),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(RepositoryRef {
            id: c.id("id")?,
            repository_id: c.id("repository_id")?,
            name: c.text("name")?,
            sha: c.text("sha")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for RepositoryDependency {
    const TABLE: Table = Table::RepositoryDependency;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.from_repository_id),
            id_sql(self.to_repository_id),
            text_sql(&self.kind),
            text_sql(&self.description),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(RepositoryDependency {
            id: c.id("id")?,
            from_repository_id: c.id("from_repository_id")?,
            to_repository_id: c.id("to_repository_id")?,
            kind: c.text("kind")?,
            description: c.text("description")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for Task {
    const TABLE: Table = Table::Task;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            Sql::Integer(self.number),
            Sql::Text(self.key.to_string()),
            text_sql(&self.title),
            text_sql(&self.description),
            text_sql(&self.description_hash),
            strings_sql(&self.acceptance_criteria)?,
            text_sql(self.status.as_str()),
            text_sql(self.priority.as_str()),
            opt_text_sql(&self.category),
            opt_id_sql(self.parent_id),
            opt_text_sql(&self.assignee),
            opt_text_sql(&self.assigned_agent),
            opt_text_sql(&self.claimed_by),
            opt_ts_sql(self.claim_expires_at),
            bool_sql(self.auto_execute),
            opt_object_sql(&self.estimated_effort)?,
            object_sql(&self.metadata)?,
            opt_ts_sql(self.due_at),
            opt_ts_sql(self.scheduled_at),
            text_sql(&self.created_by),
            Sql::Integer(self.version),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
            opt_ts_sql(self.started_at),
            opt_ts_sql(self.completed_at),
            opt_ts_sql(self.accepted_at),
            opt_ts_sql(self.archived_at),
            opt_id_sql(self.current_classification_id),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        let key_text = c.text("key")?;
        let key = TaskKey::parse(&key_text).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
        })?;
        Ok(Task {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            number: c.int("number")?,
            key,
            title: c.text("title")?,
            description: c.text("description")?,
            description_hash: c.text("description_hash")?,
            acceptance_criteria: c.strings("acceptance_criteria")?,
            status: c.parsed("status")?,
            priority: c.parsed("priority")?,
            category: c.opt_text("category")?,
            parent_id: c.opt_id("parent_id")?,
            assignee: c.opt_text("assignee")?,
            assigned_agent: c.opt_text("assigned_agent")?,
            claimed_by: c.opt_text("claimed_by")?,
            claim_expires_at: c.opt_ts("claim_expires_at")?,
            auto_execute: c.flag("auto_execute")?,
            estimated_effort: c.opt_object("estimated_effort")?,
            metadata: c.object("metadata")?,
            due_at: c.opt_ts("due_at")?,
            scheduled_at: c.opt_ts("scheduled_at")?,
            created_by: c.text("created_by")?,
            version: c.int("version")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
            started_at: c.opt_ts("started_at")?,
            completed_at: c.opt_ts("completed_at")?,
            accepted_at: c.opt_ts("accepted_at")?,
            archived_at: c.opt_ts("archived_at")?,
            current_classification_id: c.opt_id("current_classification_id")?,
        })
    }
}

impl TableRow for TaskFile {
    const TABLE: Table = Table::TaskFile;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.task_id),
            Sql::Integer(self.position),
            text_sql(&self.path),
            opt_text_sql(&self.anchor),
            text_sql(&self.note),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(TaskFile {
            task_id: c.id("task_id")?,
            position: c.int("position")?,
            path: c.text("path")?,
            anchor: c.opt_text("anchor")?,
            note: c.text("note")?,
        })
    }
}

impl TableRow for TaskTag {
    const TABLE: Table = Table::TaskTag;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![id_sql(self.task_id), text_sql(&self.tag)])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(TaskTag {
            task_id: c.id("task_id")?,
            tag: c.text("tag")?,
        })
    }
}

impl TableRow for TaskLink {
    const TABLE: Table = Table::TaskLink;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.task_id),
            text_sql(&self.kind),
            text_sql(&self.provider),
            text_sql(&self.url),
            text_sql(&self.normalized_url),
            text_sql(&self.repo_full_name),
            opt_int_sql(self.number),
            opt_id_sql(self.repository_id),
            strings_sql(&self.sources)?,
            text_sql(&self.title),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(TaskLink {
            id: c.id("id")?,
            task_id: c.id("task_id")?,
            kind: c.text("kind")?,
            provider: c.text("provider")?,
            url: c.text("url")?,
            normalized_url: c.text("normalized_url")?,
            repo_full_name: c.text("repo_full_name")?,
            number: c.opt_int("number")?,
            repository_id: c.opt_id("repository_id")?,
            sources: c.strings("sources")?,
            title: c.text("title")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for TaskComment {
    const TABLE: Table = Table::TaskComment;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.task_id),
            opt_id_sql(self.attempt_id),
            text_sql(&self.author),
            text_sql(self.kind.as_str()),
            text_sql(&self.body),
            object_sql(&self.data)?,
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(TaskComment {
            id: c.id("id")?,
            task_id: c.id("task_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            author: c.text("author")?,
            kind: c.parsed("kind")?,
            body: c.text("body")?,
            data: c.object("data")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for TaskDependency {
    const TABLE: Table = Table::TaskDependency;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.task_id),
            id_sql(self.depends_on_id),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(TaskDependency {
            id: c.id("id")?,
            task_id: c.id("task_id")?,
            depends_on_id: c.id("depends_on_id")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for TaskRelation {
    const TABLE: Table = Table::TaskRelation;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.source_task_id),
            id_sql(self.target_task_id),
            text_sql(self.relation_type.as_str()),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(TaskRelation {
            id: c.id("id")?,
            source_task_id: c.id("source_task_id")?,
            target_task_id: c.id("target_task_id")?,
            relation_type: c.parsed("relation_type")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for TaskProject {
    const TABLE: Table = Table::TaskProject;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![id_sql(self.task_id), id_sql(self.project_id)])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(TaskProject {
            task_id: c.id("task_id")?,
            project_id: c.id("project_id")?,
        })
    }
}

impl TableRow for TaskRepository {
    const TABLE: Table = Table::TaskRepository;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.task_id),
            id_sql(self.repository_id),
            text_sql(&self.access),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(TaskRepository {
            task_id: c.id("task_id")?,
            repository_id: c.id("repository_id")?,
            access: c.text("access")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for ExternalItem {
    const TABLE: Table = Table::ExternalItem;

    fn values(&self) -> Result<Vec<Sql>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            id_sql(self.task_id),
            text_sql(&self.connection),
            text_sql(&self.external_id),
            text_sql(&self.external_key),
            text_sql(&self.url),
            text_sql(&self.tracker_status),
            text_sql(&self.tracker_status_category),
            opt_text_sql(&self.tracker_resolution),
            opt_ts_sql(self.remote_updated_at),
            object_sql(&self.snapshot)?,
            text_sql(&self.snapshot_hash),
            opt_ts_sql(self.synced_at),
            opt_ts_sql(self.missing_since),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_, '_>) -> rusqlite::Result<Self> {
        Ok(ExternalItem {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            task_id: c.id("task_id")?,
            connection: c.text("connection")?,
            external_id: c.text("external_id")?,
            external_key: c.text("external_key")?,
            url: c.text("url")?,
            tracker_status: c.text("tracker_status")?,
            tracker_status_category: c.text("tracker_status_category")?,
            tracker_resolution: c.opt_text("tracker_resolution")?,
            remote_updated_at: c.opt_ts("remote_updated_at")?,
            snapshot: c.object("snapshot")?,
            snapshot_hash: c.text("snapshot_hash")?,
            synced_at: c.opt_ts("synced_at")?,
            missing_since: c.opt_ts("missing_since")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}
