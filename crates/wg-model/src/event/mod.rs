//! Domain events: [`EventDraft`] (one row of the append-only `events` log, minus its `seq`) and
//! the typed payloads of contract CF-06 (`contracts/events.json`).
//!
//! Each payload is a struct whose fields are the event's `data` keys in the order Python builds
//! the dict, so [`EventPayload::to_data`] gives the same object Python writes. One struct may serve
//! several event types that share a shape (`task.archived` and `task.unarchived` are both
//! [`task::TaskKeyed`]); [`EventPayload`] has one variant per event type, named after it.

pub mod project;
pub mod repository;
pub mod task;
pub mod workspace;

use rpds::VectorSync;
use serde::{Serialize, Serializer};
use serde_json::Value;
use wg_types::{Actor, ErrorCode, Id, Timestamp, WgError};

use crate::entity::JsonObject;
use crate::stamp::Stamp;

pub use project::{
    ProjectCreated, ProjectRepositoryAdded, ProjectRepositoryRemoved, ProjectUpdated,
};
pub use repository::{
    RepositoryAttached, RepositoryChangesDetected, RepositoryCheckoutDiscovered,
    RepositoryCheckoutLost, RepositoryCheckoutRemoved, RepositoryDependencyDeclared,
    RepositoryHealthChanged, RepositoryRemoved, RepositoryUpdated,
};
pub use task::{
    TaskAccepted, TaskChangeSurfaceComputed, TaskClaimed, TaskCommentRecorded, TaskCommitLinked,
    TaskCommitUnlinked, TaskCompleted, TaskCreated, TaskDecomposed, TaskDependencyChanged,
    TaskEscalated, TaskKeyed, TaskRejected, TaskRelationAdded, TaskReleased, TaskReopened,
    TaskRepositoryLinked, TaskRepositoryUnlinked, TaskStatusChanged, TaskSynced, TaskUpdated,
};
pub use workspace::{WorkspaceArchived, WorkspaceCreated, WorkspaceSettingsChanged};

/// The `entity_type` column of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityType {
    Workspace,
    Project,
    Repository,
    Task,
}

impl EntityType {
    pub const fn as_str(self) -> &'static str {
        match self {
            EntityType::Workspace => "workspace",
            EntityType::Project => "project",
            EntityType::Repository => "repository",
            EntityType::Task => "task",
        }
    }
}

/// Declares [`EventPayload`]: one variant per event type with its payload struct, wire name,
/// entity type and current schema version (Python's `EVENT_SCHEMA_VERSIONS`, default 1).
macro_rules! event_payloads {
    ($( $variant:ident($payload:ty) = $name:literal, $entity:ident, version $version:literal; )+) => {
        /// The typed `data` of one event, by event type.
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum EventPayload {
            $( $variant($payload), )+
        }

        impl EventPayload {
            /// Every event type the model knows, in declaration order.
            pub const EVENT_TYPES: &'static [&'static str] = &[ $( $name ),+ ];

            /// The `event_type` column.
            pub const fn event_type(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => $name, )+
                }
            }

            /// The `entity_type` column.
            pub const fn entity_type(&self) -> EntityType {
                match self {
                    $( Self::$variant(_) => EntityType::$entity, )+
                }
            }

            /// The `schema_version` column: the current version of the type's `data` shape.
            pub const fn schema_version(&self) -> u32 {
                match self {
                    $( Self::$variant(_) => $version, )+
                }
            }

            /// The `data` object, keys in Python's order (before the reserved `_automation`
            /// key, which [`EventDraft::data`] adds).
            pub fn to_data(&self) -> Result<JsonObject, WgError> {
                match self {
                    $( Self::$variant(payload) => object(payload), )+
                }
            }
        }
    };
}

event_payloads! {
    WorkspaceCreated(WorkspaceCreated) = "workspace.created", Workspace, version 1;
    WorkspaceSettingsChanged(WorkspaceSettingsChanged) = "workspace.settings_changed", Workspace, version 1;
    WorkspaceArchived(WorkspaceArchived) = "workspace.archived", Workspace, version 1;

    ProjectCreated(ProjectCreated) = "project.created", Project, version 1;
    ProjectUpdated(ProjectUpdated) = "project.updated", Project, version 1;
    ProjectRepositoryAdded(ProjectRepositoryAdded) = "project.repository_added", Project, version 1;
    ProjectRepositoryRemoved(ProjectRepositoryRemoved) = "project.repository_removed", Project, version 1;

    RepositoryRegistered(RepositoryAttached) = "repository.registered", Repository, version 1;
    RepositoryCheckoutAdded(RepositoryAttached) = "repository.checkout_added", Repository, version 1;
    RepositoryCheckoutDiscovered(RepositoryCheckoutDiscovered) = "repository.checkout_discovered", Repository, version 1;
    RepositoryCheckoutLost(RepositoryCheckoutLost) = "repository.checkout_lost", Repository, version 1;
    RepositoryCheckoutRemoved(RepositoryCheckoutRemoved) = "repository.checkout_removed", Repository, version 1;
    RepositoryChangesDetected(RepositoryChangesDetected) = "repository.changes_detected", Repository, version 1;
    RepositoryDependencyDeclared(RepositoryDependencyDeclared) = "repository.dependency_declared", Repository, version 1;
    RepositoryHealthChanged(RepositoryHealthChanged) = "repository.health_changed", Repository, version 1;
    RepositoryRemoved(RepositoryRemoved) = "repository.removed", Repository, version 1;
    RepositoryUpdated(RepositoryUpdated) = "repository.updated", Repository, version 1;

    TaskCreated(TaskCreated) = "task.created", Task, version 2;
    TaskUpdated(TaskUpdated) = "task.updated", Task, version 1;
    TaskStatusChanged(TaskStatusChanged) = "task.status_changed", Task, version 1;
    TaskReopened(TaskReopened) = "task.reopened", Task, version 1;
    TaskBecameReady(TaskKeyed) = "task.became_ready", Task, version 1;
    TaskArchived(TaskKeyed) = "task.archived", Task, version 1;
    TaskUnarchived(TaskKeyed) = "task.unarchived", Task, version 1;
    TaskClaimed(TaskClaimed) = "task.claimed", Task, version 1;
    TaskReleased(TaskReleased) = "task.released", Task, version 1;
    TaskCompleted(TaskCompleted) = "task.completed", Task, version 1;
    TaskCommentAdded(TaskCommentRecorded) = "task.comment_added", Task, version 1;
    TaskProgressReported(TaskCommentRecorded) = "task.progress_reported", Task, version 1;
    TaskResultReported(TaskCommentRecorded) = "task.result_reported", Task, version 1;
    TaskDependencyAdded(TaskDependencyChanged) = "task.dependency_added", Task, version 1;
    TaskDependencyRemoved(TaskDependencyChanged) = "task.dependency_removed", Task, version 1;
    TaskRelationAdded(TaskRelationAdded) = "task.relation_added", Task, version 1;
    TaskRepositoryLinked(TaskRepositoryLinked) = "task.repository_linked", Task, version 1;
    TaskRepositoryUnlinked(TaskRepositoryUnlinked) = "task.repository_unlinked", Task, version 1;
    TaskAccepted(TaskAccepted) = "task.accepted", Task, version 1;
    TaskRejected(TaskRejected) = "task.rejected", Task, version 1;
    TaskEscalated(TaskEscalated) = "task.escalated", Task, version 1;
    TaskDecomposed(TaskDecomposed) = "task.decomposed", Task, version 1;
    TaskSynced(TaskSynced) = "task.synced", Task, version 1;
    TaskCommitLinked(TaskCommitLinked) = "task.commit_linked", Task, version 1;
    TaskCommitUnlinked(TaskCommitUnlinked) = "task.commit_unlinked", Task, version 1;
    TaskChangeSurfaceComputed(TaskChangeSurfaceComputed) = "task.change_surface_computed", Task, version 1;
}

fn object<T: Serialize>(payload: &T) -> Result<JsonObject, WgError> {
    match serde_json::to_value(payload) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("event data is not an object: {other}"),
        )),
        Err(err) => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("event data does not serialise: {err}"),
        )),
    }
}

/// One event to append: every column of an `events` row except `seq`, which SQLite assigns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDraft {
    pub id: Id,
    pub workspace_id: Option<Id>,
    pub entity_id: Id,
    /// `actor` and `actor_kind` columns.
    pub actor: Actor,
    pub occurred_at: Timestamp,
    pub correlation_id: Id,
    pub causation_id: Option<Id>,
    pub attempt_id: Option<Id>,
    /// Non-zero inside automation runs: [`EventDraft::data`] then adds the `_automation` key.
    pub automation_depth: u32,
    pub payload: EventPayload,
}

impl EventDraft {
    /// An event stamped with the decision's actor, clock, correlation, causation, attempt and
    /// automation depth (Python's `record_event` with `ctx`).
    pub fn new(
        stamp: &Stamp,
        id: Id,
        workspace_id: Option<Id>,
        entity_id: Id,
        payload: EventPayload,
    ) -> Self {
        EventDraft {
            id,
            workspace_id,
            entity_id,
            actor: stamp.actor.clone(),
            occurred_at: stamp.at,
            correlation_id: stamp.correlation,
            causation_id: stamp.causation,
            attempt_id: stamp.attempt,
            automation_depth: stamp.automation_depth,
            payload,
        }
    }

    /// `ctx.caused_by(event_id)`: marks this event as a consequence of another.
    #[must_use]
    pub fn caused_by(self, event_id: Id) -> Self {
        EventDraft {
            causation_id: Some(event_id),
            ..self
        }
    }

    /// `attempt_id or ctx.attempt_id`: an explicit attempt overrides the stamp's.
    #[must_use]
    pub fn with_attempt(self, attempt_id: Option<Id>) -> Self {
        EventDraft {
            attempt_id: attempt_id.or(self.attempt_id),
            ..self
        }
    }

    pub fn event_type(&self) -> &'static str {
        self.payload.event_type()
    }

    pub fn entity_type(&self) -> EntityType {
        self.payload.entity_type()
    }

    pub fn schema_version(&self) -> u32 {
        self.payload.schema_version()
    }

    /// The `data` column: the payload object, plus `_automation: {depth, actor}` inside
    /// automation runs (replacing a payload key of that name in place, as Python's dict does).
    pub fn data(&self) -> Result<JsonObject, WgError> {
        let base = self.payload.to_data()?;
        if self.automation_depth == 0 {
            return Ok(base);
        }
        let mark: JsonObject = [
            ("depth".to_owned(), Value::from(self.automation_depth)),
            ("actor".to_owned(), Value::String(self.actor.to_string())),
        ]
        .into_iter()
        .collect();
        Ok(base
            .into_iter()
            .chain(core::iter::once((
                "_automation".to_owned(),
                Value::Object(mark),
            )))
            .collect())
    }
}

/// A `changes` object: field name to `[before, after]`, in the order the fields were compared.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changes(pub VectorSync<FieldChange>);

/// One entry of [`Changes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    pub field: String,
    pub before: Value,
    pub after: Value,
}

impl Changes {
    pub fn new() -> Self {
        Changes::default()
    }

    /// Adds `field: [before, after]` at the end.
    #[must_use]
    pub fn with(self, field: &str, before: Value, after: Value) -> Self {
        Changes(self.0.push_back(FieldChange {
            field: field.to_owned(),
            before,
            after,
        }))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for Changes {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(
            self.0
                .iter()
                .map(|change| (&change.field, [&change.before, &change.after])),
        )
    }
}

/// A JSON object of string values in a given order (git remotes: name to URL).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StrMap(pub VectorSync<(String, String)>);

impl Serialize for StrMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(key, value)| (key, value)))
    }
}

/// `serialize_with` helpers for wg-types values that have no serde form of their own.
pub(crate) mod ser {
    use rpds::VectorSync;
    use serde::Serializer;
    use wg_types::{TaskKey, Timestamp};

    pub(crate) fn task_key<S: Serializer>(key: &TaskKey, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(key)
    }

    pub(crate) fn task_keys<S: Serializer>(
        keys: &VectorSync<TaskKey>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(keys.iter().map(ToString::to_string))
    }

    /// Python `datetime.isoformat()` of a UTC value.
    pub(crate) fn isoformat<S: Serializer>(
        at: &Timestamp,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&at.format_isoformat())
    }
}
