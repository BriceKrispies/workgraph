//! [`IntelSlice`]: classifications, gold labels, evaluations and task embeddings.

use rpds::RedBlackTreeMapSync;
use wg_types::Id;

use crate::entity::{
    Classification, ClassificationEvaluation, ClassificationGoldLabel, TaskEmbedding,
};
use crate::write::{Row, RowKey, Write};

/// Loaded rows of the classification family, keyed by primary key.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntelSlice {
    pub classifications: RedBlackTreeMapSync<Id, Classification>,
    pub gold_labels: RedBlackTreeMapSync<Id, ClassificationGoldLabel>,
    pub evaluations: RedBlackTreeMapSync<Id, ClassificationEvaluation>,
    pub embeddings: RedBlackTreeMapSync<Id, TaskEmbedding>,
}

impl IntelSlice {
    /// The classifications of a task, by classification id.
    pub fn classifications_of(&self, task_id: Id) -> impl Iterator<Item = &Classification> {
        self.classifications
            .values()
            .filter(move |row| row.task_id == task_id)
    }

    /// The classification a task's category is read from (`is_current`), if one is loaded.
    pub fn current_classification(&self, task_id: Id) -> Option<&Classification> {
        self.classifications_of(task_id).find(|row| row.is_current)
    }

    /// The slice after `write`. Writes to tables this slice does not hold leave it unchanged.
    #[must_use]
    pub fn apply(&self, write: &Write) -> Self {
        match write {
            Write::Insert(row) | Write::Update { row, .. } => match row {
                Row::Classification(row) => IntelSlice {
                    classifications: self.classifications.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::ClassificationGoldLabel(row) => IntelSlice {
                    gold_labels: self.gold_labels.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::ClassificationEvaluation(row) => IntelSlice {
                    evaluations: self.evaluations.insert(row.id, row.clone()),
                    ..self.clone()
                },
                Row::TaskEmbedding(row) => IntelSlice {
                    embeddings: self.embeddings.insert(row.id, row.clone()),
                    ..self.clone()
                },
                _ => self.clone(),
            },
            Write::Delete(key) => match key {
                RowKey::Classification(id) => IntelSlice {
                    classifications: self.classifications.remove(id),
                    ..self.clone()
                },
                RowKey::ClassificationGoldLabel(id) => IntelSlice {
                    gold_labels: self.gold_labels.remove(id),
                    ..self.clone()
                },
                RowKey::ClassificationEvaluation(id) => IntelSlice {
                    evaluations: self.evaluations.remove(id),
                    ..self.clone()
                },
                RowKey::TaskEmbedding(id) => IntelSlice {
                    embeddings: self.embeddings.remove(id),
                    ..self.clone()
                },
                _ => self.clone(),
            },
        }
    }
}
