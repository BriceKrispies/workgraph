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
pub mod repository;
pub mod session;
pub mod task;
pub mod verification;
pub mod workspace;

/// A JSON object column (`settings`, `metadata`, `health_detail`, ...): stored as O10 text,
/// carried as an ordered map. Opaque to the model; the core reads the keys it knows.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

/// A `REAL` column (`confidence`, `accuracy`, `metric_value`). Compared by bit pattern so that rows
/// keep `Eq`; the values Python writes are finite, and bit equality is the round-trip test.
#[derive(Debug, Clone, Copy)]
pub struct Real(pub f64);

impl PartialEq for Real {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Real {}

impl serde::Serialize for Real {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

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
pub use repository::{
    Repository, RepositoryCheckout, RepositoryDependency, RepositoryRef, RepositoryRemote,
};
pub use session::{
    AgentInternalTask, AgentProfile, AgentSession, HookEvent, SessionBinding, SubagentRun,
    ToolEvent,
};
pub use task::{
    ExternalItem, Task, TaskComment, TaskDependency, TaskFile, TaskLink, TaskProject, TaskRelation,
    TaskRepository, TaskTag,
};
pub use verification::{
    TaskAcceptance, VerificationCheckResult, VerificationConfig, VerificationRun,
};
pub use workspace::Workspace;
