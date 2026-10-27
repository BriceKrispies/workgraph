//! Bulk task updates: each task commits or fails on its own, and the result names the updated
//! keys and the per-reference failures. Mirrors `bulk_update` in `core/services/tasks.py`.

use wg_types::{ErrorCode, WgError};

/// Error codes a bulk update records per task. Any other code aborts the batch, as in Python
/// where only the `WorkgraphError` subclasses named there are caught.
pub const RECORDED_CODES: &[ErrorCode] = &[
    ErrorCode::ValidationFailed,
    ErrorCode::DependencyCycle,
    ErrorCode::Conflict,
    ErrorCode::VersionConflict,
    ErrorCode::InvalidTransition,
    ErrorCode::ResourceBusy,
    ErrorCode::NotFound,
    ErrorCode::PermissionDenied,
];

/// The outcome of a bulk update (`BulkResult`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BulkResult {
    /// Canonical keys of the tasks that were updated, in request order.
    pub updated: Vec<String>,
    /// Message per requested reference that failed, in first-request order. A reference listed
    /// twice keeps its first position and its last message, as a Python dict does.
    pub failed: Vec<(String, String)>,
}

/// Folds the per-task outcomes of one bulk update, given in request order as
/// `(reference, outcome)`. An outcome carries the task's canonical key on success.
pub fn bulk_results(
    outcomes: impl IntoIterator<Item = (String, Result<String, WgError>)>,
) -> Result<BulkResult, WgError> {
    outcomes
        .into_iter()
        .try_fold(BulkResult::default(), |result, (reference, outcome)| {
            let BulkResult { updated, failed } = result;
            match outcome {
                Ok(key) => Ok(BulkResult {
                    updated: updated.into_iter().chain([key]).collect(),
                    failed,
                }),
                Err(error) if RECORDED_CODES.contains(&error.code) => {
                    let seen = failed.iter().any(|(seen, _)| *seen == reference);
                    let failed = if seen {
                        // Keep the first position, take the last message.
                        failed
                            .into_iter()
                            .map(|(seen, message)| {
                                if seen == reference {
                                    (seen, error.message.clone())
                                } else {
                                    (seen, message)
                                }
                            })
                            .collect()
                    } else {
                        failed
                            .into_iter()
                            .chain([(reference, error.message)])
                            .collect()
                    };
                    Ok(BulkResult { updated, failed })
                }
                Err(error) => Err(error),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_recorded_and_other_codes_abort() {
        let outcomes = vec![
            ("WG-1".to_owned(), Ok("WG-1".to_owned())),
            (
                "WG-404".to_owned(),
                Err(WgError::not_found("task WG-404 not found")),
            ),
            ("WG-2".to_owned(), Ok("WG-2".to_owned())),
        ];
        let result = bulk_results(outcomes).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(result.updated, vec!["WG-1", "WG-2"]);
        assert_eq!(
            result.failed,
            vec![("WG-404".to_owned(), "task WG-404 not found".to_owned())]
        );

        let fatal = vec![("X".to_owned(), Err(WgError::git("boom")))];
        assert_eq!(
            bulk_results(fatal).err().map(|e| e.code),
            Some(ErrorCode::GitError)
        );
    }
}
