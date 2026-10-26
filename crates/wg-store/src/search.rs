//! Search-family reads on a [`Session`] (SH-11): the bm25 candidates of the lexical search, the
//! task ids that pass the search filters, and the embedding reads.
//!
//! The statements come from `wg_storecodec::search_sql`; this module runs them and decodes the
//! rows. Building the MATCH expression and combining the rankings stay with the callers.

use wg_model::entity::TaskEmbedding;
use wg_storecodec::TableRow;
use wg_storecodec::search_sql::{self, FtsCandidate, SearchScope, TaskVector};
use wg_types::{Id, WgError};

use crate::Session;

impl Session<'_> {
    /// The bm25 candidates of `match_expr` in `workspace_id`, best first (`limit * 5` at most).
    pub fn fts_candidates(
        &mut self,
        match_expr: &str,
        workspace_id: Id,
        limit: i64,
    ) -> Result<Vec<FtsCandidate>, WgError> {
        self.rows(&search_sql::lexical_candidates(
            match_expr,
            workspace_id,
            limit,
        ))?
        .iter()
        .map(|values| FtsCandidate::from_raw(values))
        .collect()
    }

    /// The ids of the tasks of `workspace_id` that pass `scope`.
    pub fn scoped_task_ids(
        &mut self,
        workspace_id: Id,
        scope: &SearchScope,
    ) -> Result<Vec<Id>, WgError> {
        self.rows(&search_sql::search_scope(workspace_id, scope))?
            .iter()
            .map(|values| search_sql::scoped_task_id(values))
            .collect()
    }

    /// The embedding `task_id` has under `provider`/`model`, if one is stored.
    pub fn task_embedding(
        &mut self,
        task_id: Id,
        provider: &str,
        model: &str,
    ) -> Result<Option<TaskEmbedding>, WgError> {
        self.rows(&search_sql::task_embedding(task_id, provider, model))?
            .first()
            .map(|values| TaskEmbedding::from_raw(values))
            .transpose()
    }

    /// Every stored vector of `workspace_id` under `provider`/`model`.
    pub fn embedding_vectors(
        &mut self,
        workspace_id: Id,
        provider: &str,
        model: &str,
    ) -> Result<Vec<TaskVector>, WgError> {
        self.rows(&search_sql::embedding_vectors(
            workspace_id,
            provider,
            model,
        ))?
        .iter()
        .map(|values| TaskVector::from_raw(values))
        .collect()
    }
}
