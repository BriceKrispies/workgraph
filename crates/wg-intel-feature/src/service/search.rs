//! Search-family reads on a [`Session`] (SH-11): the bm25 candidates of the lexical search, the
//! task ids that pass the search filters, and the embedding reads.
//!
//! The statements come from [`crate::spec::search_sql`]; this module runs them and decodes the
//! rows. Building the MATCH expression and combining the rankings stay with the callers.

use crate::spec::search_sql::{self, FtsCandidate, SearchScope, TaskVector};
use wg_model::entity::TaskEmbedding;
use wg_storecodec::TableRow;
use wg_types::{Id, WgError};

use wg_store::Session;

/// The bm25 candidates of `match_expr` in `workspace_id`, best first (`limit * 5` at most).
pub fn fts_candidates(
    session: &mut Session<'_>,
    match_expr: &str,
    workspace_id: Id,
    limit: i64,
) -> Result<Vec<FtsCandidate>, WgError> {
    session
        .rows(&search_sql::lexical_candidates(
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
    session: &mut Session<'_>,
    workspace_id: Id,
    scope: &SearchScope,
) -> Result<Vec<Id>, WgError> {
    session
        .rows(&search_sql::search_scope(workspace_id, scope))?
        .iter()
        .map(|values| search_sql::scoped_task_id(values))
        .collect()
}

/// The embedding `task_id` has under `provider`/`model`, if one is stored.
pub fn task_embedding(
    session: &mut Session<'_>,
    task_id: Id,
    provider: &str,
    model: &str,
) -> Result<Option<TaskEmbedding>, WgError> {
    session
        .rows(&search_sql::task_embedding(task_id, provider, model))?
        .first()
        .map(|values| TaskEmbedding::from_raw(values))
        .transpose()
}

/// Every stored vector of `workspace_id` under `provider`/`model`.
pub fn embedding_vectors(
    session: &mut Session<'_>,
    workspace_id: Id,
    provider: &str,
    model: &str,
) -> Result<Vec<TaskVector>, WgError> {
    session
        .rows(&search_sql::embedding_vectors(
            workspace_id,
            provider,
            model,
        ))?
        .iter()
        .map(|values| TaskVector::from_raw(values))
        .collect()
}
