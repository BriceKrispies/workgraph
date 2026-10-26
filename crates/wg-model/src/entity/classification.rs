//! The classification family: `classifications`, `classification_gold_labels`,
//! `classification_evaluations` and `task_embeddings`.

use rpds::VectorSync;
use wg_types::{Id, Timestamp};

use crate::entity::{JsonObject, Real};
use crate::write::{Row, RowKey};

/// A row of `classifications`: one classification of a task. `is_current` marks the one the
/// task's category is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub id: Id,
    pub task_id: Id,
    /// `model` or `manual`.
    pub source: String,
    pub status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt_version: Option<String>,
    pub taxonomy_version: String,
    pub input_hash: String,
    pub category: Option<String>,
    pub subtype: Option<String>,
    pub domain: Option<String>,
    /// JSON list of affected system names.
    pub affected_systems: VectorSync<String>,
    pub complexity: Option<String>,
    /// JSON list of tags.
    pub tags: VectorSync<String>,
    pub confidence: Option<Real>,
    pub rationale: Option<String>,
    pub raw_output: Option<JsonObject>,
    pub validation_status: String,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub is_current: bool,
    pub confirmed_by: Option<String>,
    pub batch_id: Option<Id>,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// A row of `classification_gold_labels`: a human-labelled example for evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationGoldLabel {
    pub id: Id,
    pub workspace_id: Id,
    /// Null once the task is deleted (`ON DELETE SET NULL`).
    pub task_id: Option<Id>,
    pub title: String,
    pub description: String,
    pub category: String,
    pub labeled_by: String,
    pub created_at: Timestamp,
}

/// A row of `classification_evaluations`: the accuracy of one provider, model and prompt run over
/// the gold labels of a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationEvaluation {
    pub id: Id,
    pub workspace_id: Id,
    pub provider: String,
    pub model: String,
    pub prompt_version: String,
    pub taxonomy_version: String,
    pub sample_size: i64,
    pub failures: i64,
    pub accuracy: Option<Real>,
    pub macro_f1: Option<Real>,
    pub per_class: JsonObject,
    pub confusion: JsonObject,
    /// JSON list of per-label result objects.
    pub results: VectorSync<JsonObject>,
    pub created_at: Timestamp,
}

/// A row of `task_embeddings`: the embedding vector of a task's text under one provider and model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEmbedding {
    pub id: Id,
    pub task_id: Id,
    pub provider: String,
    pub model: String,
    pub dimensions: i64,
    /// The `BLOB` column: the packed vector bytes.
    pub vector: VectorSync<u8>,
    pub input_hash: String,
    pub created_at: Timestamp,
}

impl Classification {
    pub fn row_key(&self) -> RowKey {
        RowKey::Classification(self.id)
    }
}

impl ClassificationGoldLabel {
    pub fn row_key(&self) -> RowKey {
        RowKey::ClassificationGoldLabel(self.id)
    }
}

impl ClassificationEvaluation {
    pub fn row_key(&self) -> RowKey {
        RowKey::ClassificationEvaluation(self.id)
    }
}

impl TaskEmbedding {
    pub fn row_key(&self) -> RowKey {
        RowKey::TaskEmbedding(self.id)
    }
}

impl From<Classification> for Row {
    fn from(row: Classification) -> Self {
        Row::Classification(row)
    }
}

impl From<ClassificationGoldLabel> for Row {
    fn from(row: ClassificationGoldLabel) -> Self {
        Row::ClassificationGoldLabel(row)
    }
}

impl From<ClassificationEvaluation> for Row {
    fn from(row: ClassificationEvaluation) -> Self {
        Row::ClassificationEvaluation(row)
    }
}

impl From<TaskEmbedding> for Row {
    fn from(row: TaskEmbedding) -> Self {
        Row::TaskEmbedding(row)
    }
}
