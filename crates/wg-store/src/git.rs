//! Reads for the git family (SH-09): commits and the rows that hang off them (file changes,
//! symbols, analyses, equivalences, task links), the object gaps of a repository, its commit
//! reachability, and the change surfaces a task leaves in a repository.
//!
//! The statements come from `wg_storecodec::git_sql`; this module only runs them through the
//! session and decodes the rows. Writes go through the generic [`Session::apply_write`] and
//! [`Session::apply_decision`] like every other family.

use wg_model::entity::{
    Commit, CommitAnalysis, CommitEquivalence, CommitFileChange, CommitSymbol, CommitTaskLink,
    GitObjectGap, TaskChangeSurface,
};
use wg_storecodec::git_sql::{
    CommitReachability, CommitTaskLinkWithKey, PathTotal, TaskCommitLink,
};
use wg_storecodec::{Raw, Statement, TableRow, git_sql};
use wg_types::{Id, Timestamp, WgError};

use crate::Session;
use crate::query::many_with;

/// Runs `statement` and decodes the first row, if there is one.
fn first<T>(
    session: &mut Session<'_>,
    statement: &Statement,
    decode: impl Fn(&[Raw]) -> Result<T, WgError>,
) -> Result<Option<T>, WgError> {
    Ok(many_with(session, statement, decode)?.into_iter().next())
}

impl Session<'_> {
    /// The commit of `repository_id` whose SHA is exactly `sha`, if one has been ingested.
    pub fn commit_by_sha(
        &mut self,
        repository_id: Id,
        sha: &str,
    ) -> Result<Option<Commit>, WgError> {
        first(
            self,
            &git_sql::commit_by_sha(repository_id, sha),
            Commit::from_raw,
        )
    }

    /// The commits of `repository_id` whose SHA starts with `prefix`, by SHA.
    pub fn commits_by_sha_prefix(
        &mut self,
        repository_id: Id,
        prefix: &str,
    ) -> Result<Vec<Commit>, WgError> {
        many_with(
            self,
            &git_sql::commits_by_sha_prefix(repository_id, prefix),
            Commit::from_raw,
        )
    }

    /// Every SHA the repository has a commit row for, in SHA order.
    pub fn commit_shas(&mut self, repository_id: Id) -> Result<Vec<String>, WgError> {
        many_with(
            self,
            &git_sql::commit_shas(repository_id),
            git_sql::commit_sha,
        )
    }

    /// The commits with the given ids, by id.
    pub fn commits_by_ids(&mut self, ids: &[Id]) -> Result<Vec<Commit>, WgError> {
        many_with(self, &git_sql::commits_by_ids(ids), Commit::from_raw)
    }

    /// How many commits the repository's commit list holds, under the same filter as
    /// [`Session::repository_commits_page`].
    pub fn repository_commits_count(
        &mut self,
        repository_id: Id,
        reachable_only: bool,
        linked_only: bool,
    ) -> Result<i64, WgError> {
        let statement =
            git_sql::repository_commits_count(repository_id, reachable_only, linked_only);
        Ok(first(self, &statement, git_sql::count)?.unwrap_or(0))
    }

    /// One page of the repository's commit list, newest first.
    pub fn repository_commits_page(
        &mut self,
        repository_id: Id,
        reachable_only: bool,
        linked_only: bool,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Commit>, WgError> {
        let statement = git_sql::repository_commits_page(
            repository_id,
            reachable_only,
            linked_only,
            limit,
            offset,
        );
        many_with(self, &statement, Commit::from_raw)
    }

    /// The task links of `commit_ids`, each with its task's key, by commit then link order.
    pub fn links_with_task_keys(
        &mut self,
        commit_ids: &[Id],
    ) -> Result<Vec<CommitTaskLinkWithKey>, WgError> {
        many_with(
            self,
            &git_sql::links_with_task_keys(commit_ids),
            CommitTaskLinkWithKey::from_raw,
        )
    }

    /// The links between `commit_id` and `task_id`, oldest first; only those from `source` when
    /// one is given.
    pub fn commit_task_links_between(
        &mut self,
        commit_id: Id,
        task_id: Id,
        source: Option<&str>,
    ) -> Result<Vec<CommitTaskLink>, WgError> {
        many_with(
            self,
            &git_sql::commit_task_links_between(commit_id, task_id, source),
            CommitTaskLink::from_raw,
        )
    }

    /// The links of `task_id` at or above `min_confidence`, each with its commit and repository
    /// name, ordered by commit date.
    pub fn task_commit_links(
        &mut self,
        task_id: Id,
        min_confidence: f64,
    ) -> Result<Vec<TaskCommitLink>, WgError> {
        many_with(
            self,
            &git_sql::task_commit_links(task_id, min_confidence),
            TaskCommitLink::from_raw,
        )
    }

    /// The file changes of `commit_ids`, by commit then path.
    pub fn file_changes(&mut self, commit_ids: &[Id]) -> Result<Vec<CommitFileChange>, WgError> {
        many_with(
            self,
            &git_sql::file_changes(commit_ids),
            CommitFileChange::from_raw,
        )
    }

    /// The file changes of `commit_ids` summed per path, by path.
    pub fn path_totals(&mut self, commit_ids: &[Id]) -> Result<Vec<PathTotal>, WgError> {
        many_with(self, &git_sql::path_totals(commit_ids), PathTotal::from_raw)
    }

    /// The symbols of `file_change_ids`, by file change then id.
    pub fn commit_symbols(&mut self, file_change_ids: &[Id]) -> Result<Vec<CommitSymbol>, WgError> {
        many_with(
            self,
            &git_sql::symbols(file_change_ids),
            CommitSymbol::from_raw,
        )
    }

    /// The structure analyses of `commit_ids`, by commit.
    pub fn commit_analyses(&mut self, commit_ids: &[Id]) -> Result<Vec<CommitAnalysis>, WgError> {
        many_with(
            self,
            &git_sql::analyses(commit_ids),
            CommitAnalysis::from_raw,
        )
    }

    /// The equivalences that start or end at `commit_id`, oldest first.
    pub fn equivalences_of_commit(
        &mut self,
        commit_id: Id,
    ) -> Result<Vec<CommitEquivalence>, WgError> {
        many_with(
            self,
            &git_sql::equivalences_of_commit(commit_id),
            CommitEquivalence::from_raw,
        )
    }

    /// The equivalence from `original` to `equivalent`, if one is recorded.
    pub fn equivalence(
        &mut self,
        original: Id,
        equivalent: Id,
    ) -> Result<Option<CommitEquivalence>, WgError> {
        first(
            self,
            &git_sql::equivalence(original, equivalent),
            CommitEquivalence::from_raw,
        )
    }

    /// The commits of `repository_id` with patch id `patch_id` other than `excluding`, by SHA.
    pub fn commits_with_patch_id(
        &mut self,
        repository_id: Id,
        patch_id: &str,
        excluding: Id,
    ) -> Result<Vec<Commit>, WgError> {
        many_with(
            self,
            &git_sql::commits_with_patch_id(repository_id, patch_id, excluding),
            Commit::from_raw,
        )
    }

    /// The unreachable commits of `repository_id` with the given author email, author date and
    /// subject, other than `excluding`, by SHA.
    pub fn unreachable_commits_matching(
        &mut self,
        repository_id: Id,
        author_email: &str,
        authored_at: Option<Timestamp>,
        subject: &str,
        excluding: Id,
    ) -> Result<Vec<Commit>, WgError> {
        many_with(
            self,
            &git_sql::unreachable_commits_matching(
                repository_id,
                author_email,
                authored_at,
                subject,
                excluding,
            ),
            Commit::from_raw,
        )
    }

    /// The object gap of `repository_id` for `sha`, if one is recorded.
    pub fn git_object_gap(
        &mut self,
        repository_id: Id,
        sha: &str,
    ) -> Result<Option<GitObjectGap>, WgError> {
        first(
            self,
            &git_sql::git_object_gap(repository_id, sha),
            GitObjectGap::from_raw,
        )
    }

    /// Every object gap of `repository_id`, in detection order.
    pub fn git_object_gaps(&mut self, repository_id: Id) -> Result<Vec<GitObjectGap>, WgError> {
        many_with(
            self,
            &git_sql::git_object_gaps(repository_id),
            GitObjectGap::from_raw,
        )
    }

    /// Every commit of `repository_id` with its id, SHA and reachability, by SHA.
    pub fn commit_reachability(
        &mut self,
        repository_id: Id,
    ) -> Result<Vec<CommitReachability>, WgError> {
        many_with(
            self,
            &git_sql::commit_reachability(repository_id),
            CommitReachability::from_raw,
        )
    }

    /// The change surfaces of `task_id`, one per repository.
    pub fn task_change_surfaces(&mut self, task_id: Id) -> Result<Vec<TaskChangeSurface>, WgError> {
        many_with(
            self,
            &git_sql::task_change_surfaces(task_id),
            TaskChangeSurface::from_raw,
        )
    }

    /// The change surface `task_id` leaves in `repository_id`, if one is recorded.
    pub fn task_change_surface(
        &mut self,
        task_id: Id,
        repository_id: Id,
    ) -> Result<Option<TaskChangeSurface>, WgError> {
        first(
            self,
            &git_sql::task_change_surface(task_id, repository_id),
            TaskChangeSurface::from_raw,
        )
    }
}
