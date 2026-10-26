//! Entity values: one struct per table, one module per table family.
//!
//! A family is a root table and the tables whose rows belong to it (their foreign key to the root
//! cascades): `workspace`, `project` (with `project_repositories`), `repository` (checkouts,
//! remotes, refs, declared dependencies) and `task` (files, tags, links, comments, dependencies,
//! relations, project and repository memberships, the tracker's external item). Fields are the
//! table's columns, in `contracts/schema-0010.sql` order; see the crate documentation for the
//! column-type mapping.

pub mod automation;
pub mod classification;
pub mod experiment;
pub mod git;
pub mod project;
pub mod queue;
pub mod real;
pub mod repository;
pub mod run;
pub mod session;
pub mod stack;
pub mod task;
pub mod verification;
pub mod workspace;

/// A JSON object column (`settings`, `metadata`, `health_detail`, ...): stored as O10 text,
/// carried as an ordered map. Opaque to the model; the core reads the keys it knows.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

pub use automation::{AutomationRule, AutomationRun, Finding, Notification, Report};
pub use classification::{
    Classification, ClassificationEvaluation, ClassificationGoldLabel, TaskEmbedding,
};
pub use experiment::{Experiment, ExperimentTrial, PredictorFit};
pub use git::{
    Commit, CommitAnalysis, CommitEquivalence, CommitFileChange, CommitSymbol, CommitTaskLink,
    GitObjectGap, TaskChangeSurface,
};
pub use project::{Project, ProjectRepository};
pub use queue::{Job, JobResourceClaim, JobRun, SystemCursor, Worker};
pub use real::Real;
pub use repository::{
    Repository, RepositoryCheckout, RepositoryDependency, RepositoryRef, RepositoryRemote,
};
pub use run::{
    AgentAttempt, Artifact, ChangeSet, ChangeSetItem, ManagedWorktree, ResourceLease,
    RevisionManifest,
};
pub use session::{
    AgentInternalTask, AgentProfile, AgentSession, HookEvent, SessionBinding, SubagentRun,
    ToolEvent,
};
pub use stack::{Stack, StackItem, StackRepository};
pub use task::{
    DecisionOption, ExternalItem, Task, TaskComment, TaskDecision, TaskDependency, TaskFile,
    TaskLink, TaskProject, TaskRelation, TaskRepository, TaskTag,
};
pub use verification::{
    TaskAcceptance, VerificationCheckResult, VerificationConfig, VerificationRun,
};
pub use workspace::Workspace;
