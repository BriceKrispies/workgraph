//! Reads for the run family (SH-06): agent attempts, revision manifests, managed worktrees,
//! resource leases, change sets and their items, and artifacts.
//!
//! The statements are built by `wg_storecodec::run_sql` as values; this module only runs them
//! through the session and decodes the rows with the table codecs. Writes go through the generic
//! [`Session::apply_write`] and [`Session::apply_decision`] like every other family.

use wg_model::entity::{
    AgentAttempt, Artifact, ChangeSet, ChangeSetItem, ManagedWorktree, ResourceLease,
    RevisionManifest,
};
use wg_storecodec::run_sql::{
    RecordedHead, WorktreeWithRepository, decode_recorded_head, decode_worktree_with_repository,
};
use wg_storecodec::{Raw, Statement, TableRow, run_sql};
use wg_types::{AttemptStatus, Id, Timestamp, WgError, WorktreeStatus};

use crate::Session;

/// One page of attempts, as the Python attempts endpoint returns it: the items, the total over
/// the whole filter, and the offset of the next page when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptPage {
    pub items: Vec<AgentAttempt>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    pub next_offset: Option<i64>,
}

/// Runs `statement` and decodes the first row, if there is one.
fn one<T: TableRow>(
    session: &mut Session<'_>,
    statement: &Statement,
) -> Result<Option<T>, WgError> {
    session
        .rows(statement)?
        .first()
        .map(|values| T::from_raw(values))
        .transpose()
}

/// Runs `statement` and decodes every row, in the order the query returns them.
fn many<T: TableRow>(session: &mut Session<'_>, statement: &Statement) -> Result<Vec<T>, WgError> {
    session
        .rows(statement)?
        .iter()
        .map(|values| T::from_raw(values))
        .collect()
}

impl Session<'_> {
    /// The attempt with `id`, if there is one.
    pub fn attempt_by_id(&mut self, id: Id) -> Result<Option<AgentAttempt>, WgError> {
        one(self, &run_sql::attempt_by_id(id))
    }

    /// The attempts of `workspace_id`, newest first, narrowed to `task_id`, `statuses` and
    /// `provider` when given (an empty `statuses` does not narrow).
    pub fn attempts_in_workspace(
        &mut self,
        workspace_id: Id,
        task_id: Option<Id>,
        statuses: &[AttemptStatus],
        provider: Option<&str>,
    ) -> Result<Vec<AgentAttempt>, WgError> {
        many(
            self,
            &run_sql::attempts_in_workspace(workspace_id, task_id, statuses, provider),
        )
    }

    /// The attempts of `task_id`, oldest number first, narrowed to `statuses` when non-empty.
    pub fn attempts_of_task(
        &mut self,
        task_id: Id,
        statuses: &[AttemptStatus],
    ) -> Result<Vec<AgentAttempt>, WgError> {
        many(self, &run_sql::attempts_of_task(task_id, statuses))
    }

    /// The revision manifest with `id`, if there is one.
    pub fn manifest_by_id(&mut self, id: Id) -> Result<Option<RevisionManifest>, WgError> {
        one(self, &run_sql::manifest_by_id(id))
    }

    /// The revision manifests of `attempt_id`, oldest first.
    pub fn manifests_of_attempt(
        &mut self,
        attempt_id: Id,
    ) -> Result<Vec<RevisionManifest>, WgError> {
        many(self, &run_sql::manifests_of_attempt(attempt_id))
    }

    /// The managed worktree at `path`, if there is one.
    pub fn worktree_by_path(&mut self, path: &str) -> Result<Option<ManagedWorktree>, WgError> {
        one(self, &run_sql::worktree_by_path(path))
    }

    /// The managed worktrees of `attempt_id` by path, narrowed to `statuses` when non-empty.
    pub fn worktrees_of_attempt(
        &mut self,
        attempt_id: Id,
        statuses: &[WorktreeStatus],
    ) -> Result<Vec<ManagedWorktree>, WgError> {
        many(self, &run_sql::worktrees_of_attempt(attempt_id, statuses))
    }

    /// The managed worktrees whose status is one of `statuses` (all of them when empty), by path.
    pub fn worktrees_with_status(
        &mut self,
        statuses: &[WorktreeStatus],
    ) -> Result<Vec<ManagedWorktree>, WgError> {
        many(self, &run_sql::worktrees_with_status(statuses))
    }

    /// The resource lease on `resource_key`, if one is held.
    pub fn lease_by_resource(
        &mut self,
        resource_key: &str,
    ) -> Result<Option<ResourceLease>, WgError> {
        one(self, &run_sql::lease_by_resource(resource_key))
    }

    /// The resource leases held by `holder`, by resource key.
    pub fn leases_held_by(&mut self, holder: &str) -> Result<Vec<ResourceLease>, WgError> {
        many(self, &run_sql::leases_held_by(holder))
    }

    /// The change set with `id`, if there is one.
    pub fn change_set_by_id(&mut self, id: Id) -> Result<Option<ChangeSet>, WgError> {
        one(self, &run_sql::change_set_by_id(id))
    }

    /// The change sets of `task_id`, oldest first.
    pub fn change_sets_of_task(&mut self, task_id: Id) -> Result<Vec<ChangeSet>, WgError> {
        many(self, &run_sql::change_sets_of_task(task_id))
    }

    /// The change sets made by `attempt_id`, oldest first.
    pub fn change_sets_of_attempt(&mut self, attempt_id: Id) -> Result<Vec<ChangeSet>, WgError> {
        many(self, &run_sql::change_sets_of_attempt(attempt_id))
    }

    /// The items of `change_set_id`, by repository id.
    pub fn change_set_items(&mut self, change_set_id: Id) -> Result<Vec<ChangeSetItem>, WgError> {
        many(self, &run_sql::change_set_items(change_set_id))
    }

    /// The artifact with `id`, if there is one.
    pub fn artifact_by_id(&mut self, id: Id) -> Result<Option<Artifact>, WgError> {
        one(self, &run_sql::artifact_by_id(id))
    }

    /// The artifacts of `task_id`, oldest first.
    pub fn artifacts_of_task(&mut self, task_id: Id) -> Result<Vec<Artifact>, WgError> {
        many(self, &run_sql::artifacts_of_task(task_id))
    }

    /// The artifacts of `attempt_id`, oldest first.
    pub fn artifacts_of_attempt(&mut self, attempt_id: Id) -> Result<Vec<Artifact>, WgError> {
        many(self, &run_sql::artifacts_of_attempt(attempt_id))
    }

    /// One page of [`Session::attempts_in_workspace`] with the total over the whole filter. The
    /// count and the page run in this session, so they see the same rows.
    pub fn attempts_page(
        &mut self,
        workspace_id: Id,
        task_id: Option<Id>,
        statuses: &[AttemptStatus],
        provider: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<AttemptPage, WgError> {
        let total = self
            .rows(&run_sql::attempts_in_workspace_count(
                workspace_id,
                task_id,
                statuses,
                provider,
            ))?
            .first()
            .and_then(|row| row.first())
            .map(|value| match value {
                Raw::Int(count) => Ok(*count),
                other => Err(WgError::new(
                    wg_types::ErrorCode::WorkgraphError,
                    format!("attempt count is not an integer: {other:?}"),
                )),
            })
            .transpose()?
            .unwrap_or(0);
        let items = many(
            self,
            &run_sql::attempts_in_workspace_page(
                workspace_id,
                task_id,
                statuses,
                provider,
                limit,
                offset,
            ),
        )?;
        Ok(AttemptPage {
            items,
            total,
            limit,
            offset,
            next_offset: run_sql::next_offset(offset, limit, total),
        })
    }

    /// The head the newest succeeded attempt of `task_id` recorded in `repository_id`, with its
    /// branch, if it touched that repository.
    pub fn latest_recorded_head(
        &mut self,
        task_id: Id,
        repository_id: Id,
    ) -> Result<Option<RecordedHead>, WgError> {
        let rows = self.rows(&run_sql::latest_recorded_head(task_id, repository_id))?;
        rows.first()
            .map(|values| decode_recorded_head(values))
            .transpose()
    }

    /// Every managed worktree, or those of `workspace_id` and `status` when given, newest first,
    /// each with its repository's name.
    pub fn worktrees_with_repository(
        &mut self,
        workspace_id: Option<Id>,
        status: Option<WorktreeStatus>,
    ) -> Result<Vec<WorktreeWithRepository>, WgError> {
        let rows = self.rows(&run_sql::worktrees_with_repository(workspace_id, status))?;
        run_sql::decode_rows(&rows, decode_worktree_with_repository)
    }

    /// The resource leases that have expired at `now`, by resource key.
    pub fn expired_leases(&mut self, now: Timestamp) -> Result<Vec<ResourceLease>, WgError> {
        many(self, &run_sql::expired_leases(now))
    }

    /// Deletes the leases that have expired at `now` and returns them, as the lease sweep does.
    pub fn sweep_expired_leases(&mut self, now: Timestamp) -> Result<Vec<ResourceLease>, WgError> {
        let expired = self.expired_leases(now)?;
        self.run(&run_sql::delete_expired_leases(now))?;
        Ok(expired)
    }

    /// The unpurged artifacts of `retention` created before `created_before`, oldest first.
    pub fn purgeable_artifacts(
        &mut self,
        retention: &str,
        created_before: Timestamp,
    ) -> Result<Vec<Artifact>, WgError> {
        many(
            self,
            &run_sql::purgeable_artifacts(retention, created_before),
        )
    }
}
