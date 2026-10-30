//! Domain events: [`EventDraft`] (one row of the append-only `events` log, minus its `seq`) and
//! the typed payloads of contract CF-06 (`contracts/events.json`).
//!
//! Each payload is a struct whose fields are the event's `data` keys in the order Python builds
//! the dict, so [`EventPayload::to_data`] gives the same object Python writes. One struct may serve
//! several event types that share a shape (`task.archived` and `task.unarchived` are both
//! [`task::TaskKeyed`]); [`EventPayload`] has one variant per event type, named after it.

pub mod attempt;
pub mod automation;
pub mod classification;
pub mod experiment;
pub mod git;
pub mod project;
pub mod repository;
pub mod session;
pub mod stack;
pub mod stored;
pub mod task;
pub mod verification;
pub mod workspace;
pub mod worktree;

use rpds::VectorSync;
use serde::{Serialize, Serializer};
use serde_json::Value;
use wg_types::{Actor, Correlation, Id, Timestamp, WgError};

use crate::entity::JsonObject;
use crate::stamp::Stamp;

pub use attempt::{
    AttemptCancelRequested, AttemptCreated, AttemptFinished, AttemptMessage,
    AttemptOutcomeCaptured, AttemptProgress, AttemptResult, AttemptStatusChanged,
    AttemptSubagentBound, AttemptUsage, AttemptWorkspacePrepared,
};
pub use automation::{
    AutomationRuleCreated, AutomationRuleToggled, AutomationRuleUpdated, AutomationRunCompleted,
    NotificationCreated, ReportGenerated,
};
pub use classification::{
    ClassificationCompleted, ClassificationConfirmed, ClassificationDisagreement,
    ClassificationEvaluated, ClassificationFailed, ClassificationPromoted,
    ClassificationSuperseded,
};
pub use experiment::{
    ExperimentClosed, ExperimentCreated, ExperimentFrozen, ExperimentProtocolUpdated,
    ExperimentTrialAdded, ExperimentTrialClassified, ExperimentTrialGraded,
    ExperimentTrialInvalidated, ExperimentTrialLinked, ExperimentTrialStarted, PredictorFitted,
};
pub use git::{GitCommitIngested, GitCommitsAttributed, GitSyncCompleted};
pub use project::{
    ProjectCreated, ProjectRepositoryAdded, ProjectRepositoryRemoved, ProjectUpdated,
};
pub use repository::{
    RepositoryAttached, RepositoryChangesDetected, RepositoryCheckoutDiscovered,
    RepositoryCheckoutLost, RepositoryCheckoutRemoved, RepositoryDependencyDeclared,
    RepositoryHealthChanged, RepositoryRemoved, RepositoryUpdated,
};
pub use session::{
    AgentInternalTaskChanged, AgentSessionBound, AgentSessionEnded, AgentSessionStarted,
    AgentSubagentChanged,
};
pub use stack::{
    StackBisectProbe, StackCreated, StackIntegrationConflict, StackIntegrationMerged,
    StackIntegrationRemoved, StackIntegrationSkipped, StackItemAdded, StackItemEjected,
    StackItemReleased, StackItemRemoved, StackItemStateChanged, StackLandRequested, StackLanded,
    StackLandingRefused, StackParentNotReviewable, StackParentRejected, StackRebased, StackRebuilt,
    StackStatusChanged, StackVerificationErrored, StackVerificationFailed,
    StackVerificationFailedOnBase, StackVerificationStarted, StackVerified,
};
pub use stored::StoredEvent;
pub use task::{
    TaskAccepted, TaskChangeSurfaceComputed, TaskClaimed, TaskCommentRecorded, TaskCommitLinked,
    TaskCommitUnlinked, TaskCompleted, TaskCreated, TaskDecisionChanged, TaskDecomposed,
    TaskDependencyChanged, TaskEscalated, TaskKeyed, TaskRejected, TaskRelationAdded, TaskReleased,
    TaskReopened, TaskRepositoryLinked, TaskRepositoryUnlinked, TaskStatusChanged, TaskSynced,
    TaskUpdated,
};
pub use verification::{
    VerificationCompleted, VerificationConfigToggled, VerificationConfigWritten,
    VerificationStarted,
};
pub use workspace::{WorkspaceArchived, WorkspaceCreated, WorkspaceSettingsChanged};
pub use worktree::{WorktreeCreated, WorktreeRecovered, WorktreeRemoved};

/// The `entity_type` column of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityType {
    Workspace,
    Project,
    Repository,
    Task,
    AgentSession,
    AutomationRule,
    AutomationRun,
    Commit,
    Experiment,
    ExperimentTrial,
    Notification,
    PredictorFit,
    Report,
    VerificationConfig,
    VerificationRun,
    Attempt,
    Stack,
    Worktree,
    System,
}

impl EntityType {
    pub const fn as_str(self) -> &'static str {
        match self {
            EntityType::Workspace => "workspace",
            EntityType::Project => "project",
            EntityType::Repository => "repository",
            EntityType::Task => "task",
            EntityType::AgentSession => "agent_session",
            EntityType::AutomationRule => "automation_rule",
            EntityType::AutomationRun => "automation_run",
            EntityType::Commit => "commit",
            EntityType::Experiment => "experiment",
            EntityType::ExperimentTrial => "experiment_trial",
            EntityType::Notification => "notification",
            EntityType::PredictorFit => "predictor_fit",
            EntityType::Report => "report",
            EntityType::VerificationConfig => "verification_config",
            EntityType::VerificationRun => "verification_run",
            EntityType::Attempt => "attempt",
            EntityType::Stack => "stack",
            EntityType::Worktree => "worktree",
            EntityType::System => "system",
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
    TaskDecisionRequested(TaskDecisionChanged) = "task.decision_requested", Task, version 1;
    TaskDecisionChosen(TaskDecisionChanged) = "task.decision_chosen", Task, version 1;
    TaskDecisionWithdrawn(TaskDecisionChanged) = "task.decision_withdrawn", Task, version 1;

    AgentSessionBound(AgentSessionBound) = "agent_session.bound", Task, version 1;
    AgentSessionStarted(AgentSessionStarted) = "agent_session.started", AgentSession, version 1;
    AgentSessionEnded(AgentSessionEnded) = "agent_session.ended", AgentSession, version 1;
    AgentSessionInternalTaskCreated(AgentInternalTaskChanged) = "agent_session.internal_task_created", AgentSession, version 1;
    AgentSessionInternalTaskCompleted(AgentInternalTaskChanged) = "agent_session.internal_task_completed", AgentSession, version 1;
    AgentSessionSubagentStarted(AgentSubagentChanged) = "agent_session.subagent_started", AgentSession, version 1;
    AgentSessionSubagentStopped(AgentSubagentChanged) = "agent_session.subagent_stopped", AgentSession, version 1;

    AutomationRuleCreated(AutomationRuleCreated) = "automation.rule_created", AutomationRule, version 1;
    AutomationRuleUpdated(AutomationRuleUpdated) = "automation.rule_updated", AutomationRule, version 1;
    AutomationRuleEnabled(AutomationRuleToggled) = "automation.rule_enabled", AutomationRule, version 1;
    AutomationRuleDisabled(AutomationRuleToggled) = "automation.rule_disabled", AutomationRule, version 1;
    AutomationRunCompleted(AutomationRunCompleted) = "automation.run_completed", AutomationRun, version 1;
    NotificationCreated(NotificationCreated) = "notification.created", Notification, version 1;
    ReportGenerated(ReportGenerated) = "report.generated", Report, version 1;

    ClassificationCompleted(ClassificationCompleted) = "classification.completed", Task, version 1;
    ClassificationConfirmed(ClassificationConfirmed) = "classification.confirmed", Task, version 1;
    ClassificationDisagreement(ClassificationDisagreement) = "classification.disagreement", Task, version 1;
    ClassificationEvaluated(ClassificationEvaluated) = "classification.evaluated", Workspace, version 1;
    ClassificationFailed(ClassificationFailed) = "classification.failed", Task, version 1;
    ClassificationPromoted(ClassificationPromoted) = "classification.promoted", Task, version 1;
    ClassificationSuperseded(ClassificationSuperseded) = "classification.superseded", Task, version 1;

    ExperimentCreated(ExperimentCreated) = "experiment.created", Experiment, version 1;
    ExperimentFrozen(ExperimentFrozen) = "experiment.frozen", Experiment, version 1;
    ExperimentProtocolUpdated(ExperimentProtocolUpdated) = "experiment.protocol_updated", Experiment, version 1;
    ExperimentClosed(ExperimentClosed) = "experiment.closed", Experiment, version 1;
    ExperimentTrialAdded(ExperimentTrialAdded) = "experiment_trial.added", ExperimentTrial, version 1;
    ExperimentTrialStarted(ExperimentTrialStarted) = "experiment_trial.started", ExperimentTrial, version 1;
    ExperimentTrialLinked(ExperimentTrialLinked) = "experiment_trial.linked", ExperimentTrial, version 1;
    ExperimentTrialGraded(ExperimentTrialGraded) = "experiment_trial.graded", ExperimentTrial, version 1;
    ExperimentTrialClassified(ExperimentTrialClassified) = "experiment_trial.classified", ExperimentTrial, version 1;
    ExperimentTrialInvalidated(ExperimentTrialInvalidated) = "experiment_trial.invalidated", ExperimentTrial, version 1;
    PredictorFitted(PredictorFitted) = "predictor.fitted", PredictorFit, version 1;

    GitCommitIngested(GitCommitIngested) = "git.commit_ingested", Commit, version 1;
    GitCommitsAttributed(GitCommitsAttributed) = "git.commits_attributed", Repository, version 1;
    GitSyncCompleted(GitSyncCompleted) = "git.sync_completed", Repository, version 1;

    VerificationConfigCreated(VerificationConfigWritten) = "verification.config_created", VerificationConfig, version 1;
    VerificationConfigUpdated(VerificationConfigWritten) = "verification.config_updated", VerificationConfig, version 1;
    VerificationConfigEnabled(VerificationConfigToggled) = "verification.config_enabled", VerificationConfig, version 1;
    VerificationConfigDisabled(VerificationConfigToggled) = "verification.config_disabled", VerificationConfig, version 1;
    VerificationStarted(VerificationStarted) = "verification.started", VerificationRun, version 1;
    VerificationCompleted(VerificationCompleted) = "verification.completed", VerificationRun, version 1;
    AttemptCreated(AttemptCreated) = "attempt.created", Attempt, version 1;
    AttemptStatusChanged(AttemptStatusChanged) = "attempt.status_changed", Attempt, version 1;
    AttemptCancelRequested(AttemptCancelRequested) = "attempt.cancel_requested", Attempt, version 1;
    AttemptSucceeded(AttemptFinished) = "attempt.succeeded", Attempt, version 1;
    AttemptFailed(AttemptFinished) = "attempt.failed", Attempt, version 1;
    AttemptCancelled(AttemptFinished) = "attempt.cancelled", Attempt, version 1;
    AttemptWorkspacePrepared(AttemptWorkspacePrepared) = "attempt.workspace_prepared", Attempt, version 1;
    AttemptOutcomeCaptured(AttemptOutcomeCaptured) = "attempt.outcome_captured", Attempt, version 1;
    AttemptSubagentBound(AttemptSubagentBound) = "attempt.subagent_bound", Attempt, version 1;
    AttemptArtifact(AttemptMessage) = "attempt.artifact", Attempt, version 1;
    AttemptLog(AttemptMessage) = "attempt.log", Attempt, version 1;
    AttemptMessage(AttemptMessage) = "attempt.message", Attempt, version 1;
    AttemptSession(AttemptMessage) = "attempt.session", Attempt, version 1;
    AttemptToolUse(AttemptMessage) = "attempt.tool_use", Attempt, version 1;
    AttemptToolResult(AttemptMessage) = "attempt.tool_result", Attempt, version 1;
    AttemptProgress(AttemptProgress) = "attempt.progress", Attempt, version 1;
    AttemptResult(AttemptResult) = "attempt.result", Attempt, version 1;
    AttemptUsage(AttemptUsage) = "attempt.usage", Attempt, version 1;

    StackCreated(StackCreated) = "stack.created", Stack, version 1;
    StackItemAdded(StackItemAdded) = "stack.item_added", Stack, version 1;
    StackItemRemoved(StackItemRemoved) = "stack.item_removed", Stack, version 1;
    StackItemReleased(StackItemReleased) = "stack.item_released", Stack, version 1;
    StackItemEjected(StackItemEjected) = "stack.item_ejected", Stack, version 1;
    StackItemStateChanged(StackItemStateChanged) = "stack.item_state_changed", Stack, version 1;
    StackStatusChanged(StackStatusChanged) = "stack.status_changed", Stack, version 1;
    StackLandRequested(StackLandRequested) = "stack.land_requested", Stack, version 1;
    StackLanded(StackLanded) = "stack.landed", Stack, version 1;
    StackLandingRefused(StackLandingRefused) = "stack.landing_refused", Stack, version 1;
    StackRebased(StackRebased) = "stack.rebased", Stack, version 1;
    StackRebuilt(StackRebuilt) = "stack.rebuilt", Stack, version 1;
    StackVerificationStarted(StackVerificationStarted) = "stack.verification_started", Stack, version 1;
    StackVerified(StackVerified) = "stack.verified", Stack, version 1;
    StackVerificationFailed(StackVerificationFailed) = "stack.verification_failed", Stack, version 1;
    StackVerificationFailedOnBase(StackVerificationFailedOnBase) = "stack.verification_failed_on_base", Stack, version 1;
    StackVerificationErrored(StackVerificationErrored) = "stack.verification_errored", Stack, version 1;
    StackBisectProbe(StackBisectProbe) = "stack.bisect_probe", Stack, version 1;
    StackIntegrationMerged(StackIntegrationMerged) = "stack.integration_merged", Stack, version 1;
    StackIntegrationConflict(StackIntegrationConflict) = "stack.integration_conflict", Stack, version 1;
    StackIntegrationSkipped(StackIntegrationSkipped) = "stack.integration_skipped", Stack, version 1;
    StackIntegrationRemoved(StackIntegrationRemoved) = "stack.integration_removed", Stack, version 1;
    StackParentNotReviewable(StackParentNotReviewable) = "stack.parent_not_reviewable", Stack, version 1;
    StackParentRejected(StackParentRejected) = "stack.parent_rejected", Stack, version 1;

    WorktreeCreated(WorktreeCreated) = "worktree.created", Worktree, version 1;
    WorktreeRemoved(WorktreeRemoved) = "worktree.removed", Worktree, version 1;
    WorktreeRecovered(WorktreeRecovered) = "worktree.recovered", System, version 1;
}

fn object<T: Serialize>(payload: &T) -> Result<JsonObject, WgError> {
    match serde_json::to_value(payload) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(WgError::workgraph(format!(
            "event data is not an object: {other}"
        ))),
        Err(err) => Err(WgError::workgraph(format!(
            "event data does not serialise: {err}"
        ))),
    }
}

/// One event to append: every column of an `events` row except `seq`, which SQLite assigns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDraft {
    pub id: Id,
    pub workspace_id: Option<Id>,
    pub entity_id: Id,
    /// The `entity_id` column when Python records text that is not an id (`worktree.recovered`
    /// writes `"worktrees"`); `entity_id` is then nil.
    pub entity_text: Option<String>,
    /// The `entity_type` column when it is not the payload's (`git.commits_attributed` of an
    /// attempt's range is recorded against the attempt).
    pub entity_type_override: Option<EntityType>,
    /// `actor` and `actor_kind` columns.
    pub actor: Actor,
    pub occurred_at: Timestamp,
    pub correlation_id: Correlation,
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
            entity_text: None,
            entity_type_override: None,
            actor: stamp.actor.clone(),
            occurred_at: stamp.at,
            correlation_id: stamp.correlation.clone(),
            causation_id: stamp.causation,
            attempt_id: stamp.attempt,
            automation_depth: stamp.automation_depth,
            payload,
        }
    }

    /// Records `text` as the entity id, for the events whose Python entity id is not an id.
    #[must_use]
    pub fn with_entity_text(self, text: impl Into<String>) -> Self {
        EventDraft {
            entity_id: Id::nil(),
            entity_text: Some(text.into()),
            ..self
        }
    }

    /// Records the event against another entity type than its payload names.
    #[must_use]
    pub fn with_entity_type(self, entity_type: EntityType) -> Self {
        EventDraft {
            entity_type_override: Some(entity_type),
            ..self
        }
    }

    /// The `entity_id` column as text.
    pub fn entity_id_text(&self) -> String {
        self.entity_text
            .clone()
            .unwrap_or_else(|| self.entity_id.to_string())
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
        self.entity_type_override
            .unwrap_or_else(|| self.payload.entity_type())
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
