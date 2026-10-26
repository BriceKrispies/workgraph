//! Classification-family reads on a [`Session`] (SH-11): the classification history of a task, the
//! reuse lookup, gold labels, evaluations, and the succeeded classifications joined to task keys.
//!
//! The statements come from [`crate::spec::classify_sql`]; this module runs them and decodes the
//! rows through the table codecs.

use crate::spec::classify_sql::{self, ClassificationWithKey};
use wg_model::entity::{Classification, ClassificationEvaluation, ClassificationGoldLabel};
use wg_storecodec::task_sql;
use wg_types::{Id, WgError};

use wg_store::Session;
use wg_store::query::many;

/// The classification with `id`, if there is one.
pub fn classification(
    session: &mut Session<'_>,
    id: Id,
) -> Result<Option<Classification>, WgError> {
    Ok(
        many::<Classification>(session, &task_sql::classification_by_id(id))?
            .into_iter()
            .next(),
    )
}

/// The classifications of `task_id`, newest first.
pub fn classification_history(
    session: &mut Session<'_>,
    task_id: Id,
) -> Result<Vec<Classification>, WgError> {
    many(session, &classify_sql::classification_history(task_id))
}

/// A succeeded classification of `task_id` made by `provider`/`model` under `prompt_version`
/// for `input_hash`, if one is stored.
pub fn succeeded_classification(
    session: &mut Session<'_>,
    task_id: Id,
    provider: &str,
    model: &str,
    prompt_version: &str,
    input_hash: &str,
) -> Result<Option<Classification>, WgError> {
    Ok(many::<Classification>(
        session,
        &classify_sql::succeeded_classification(
            task_id,
            provider,
            model,
            prompt_version,
            input_hash,
        ),
    )?
    .into_iter()
    .next())
}

/// The gold labels of `workspace_id`, oldest first.
pub fn gold_labels(
    session: &mut Session<'_>,
    workspace_id: Id,
) -> Result<Vec<ClassificationGoldLabel>, WgError> {
    many(session, &classify_sql::gold_labels(workspace_id))
}

/// The evaluations of `workspace_id`, newest first.
pub fn classification_evaluations(
    session: &mut Session<'_>,
    workspace_id: Id,
) -> Result<Vec<ClassificationEvaluation>, WgError> {
    many(
        session,
        &classify_sql::classification_evaluations(workspace_id),
    )
}

/// The succeeded classifications of `workspace_id`, each with its task's key.
pub fn classifications_with_task_keys(
    session: &mut Session<'_>,
    workspace_id: Id,
) -> Result<Vec<ClassificationWithKey>, WgError> {
    session
        .rows(&classify_sql::classifications_with_task_keys(workspace_id))?
        .iter()
        .map(|values| ClassificationWithKey::from_raw(values))
        .collect()
}
