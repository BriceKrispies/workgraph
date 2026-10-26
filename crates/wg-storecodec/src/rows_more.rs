//! Codecs for the execution and knowledge tables (generated from the model's entity structs and
//! `contracts/schema-0011.sql` by `tools/gen_storecodec_rows.py`; edit the generator, not this file).
//!
//! Each table has its `columns` list (schema order, checked against the schema by the crate's
//! tests), its primary key, and a [`TableRow`] implementation.

use wg_model::entity::*;
use wg_model::{Row, RowKey, Table};
use wg_types::WgError;

use crate::codec::{
    Cols, Raw, blob_sql, bool_sql, id_sql, object_sql, objects_sql, opt_id_sql, opt_int_sql,
    opt_object_sql, opt_real_sql, opt_text_sql, opt_ts_sql, real_sql, strings_sql, text_sql,
    ts_sql, values_sql,
};
use crate::rows::TableRow;

/// Columns of the tables beyond the first 18, in schema order; empty for those 18.
pub(crate) const fn columns_more(table: Table) -> &'static [&'static str] {
    match table {
        Table::Commit => &[
            "id",
            "repository_id",
            "sha",
            "tree_sha",
            "parents",
            "author_name",
            "author_email",
            "authored_at",
            "committer_name",
            "committer_email",
            "committed_at",
            "subject",
            "message",
            "trailers",
            "is_merge",
            "patch_id",
            "insertions",
            "deletions",
            "files_changed",
            "reachable",
            "unreachable_since",
            "object_available",
            "structure_analyzed",
            "branches",
            "ingested_at",
        ],
        Table::CommitFileChange => &[
            "id",
            "commit_id",
            "path",
            "old_path",
            "change_type",
            "similarity",
            "insertions",
            "deletions",
            "is_binary",
            "language",
            "file_class",
            "module",
        ],
        Table::CommitSymbol => &[
            "id",
            "file_change_id",
            "symbol_kind",
            "name",
            "change_kind",
            "start_line",
            "end_line",
            "lines_changed",
        ],
        Table::CommitAnalysis => &[
            "commit_id",
            "analyzer_version",
            "files_parsed",
            "files_fallback",
            "functions_affected",
            "types_affected",
            "detail",
            "analyzed_at",
        ],
        Table::CommitEquivalence => &[
            "id",
            "repository_id",
            "original_commit_id",
            "equivalent_commit_id",
            "kind",
            "evidence",
            "created_at",
        ],
        Table::CommitTaskLink => &[
            "id",
            "commit_id",
            "task_id",
            "source",
            "confidence",
            "evidence",
            "attempt_id",
            "created_by",
            "created_at",
        ],
        Table::GitObjectGap => &["id", "repository_id", "sha", "context", "detected_at"],
        Table::TaskChangeSurface => &[
            "id",
            "task_id",
            "repository_id",
            "attempt_id",
            "base_sha",
            "final_sha",
            "files_changed",
            "insertions",
            "deletions",
            "renames",
            "binary_files",
            "functions_affected",
            "types_affected",
            "detail",
            "computed_at",
            "updated_at",
        ],
        Table::VerificationConfig => &[
            "id",
            "workspace_id",
            "scope_type",
            "scope_id",
            "name",
            "checks",
            "enabled",
            "approved_by",
            "version",
            "created_at",
            "updated_at",
        ],
        Table::VerificationRun => &[
            "id",
            "workspace_id",
            "task_id",
            "attempt_id",
            "manifest_id",
            "config_snapshot",
            "status",
            "required_passed",
            "triggered_by",
            "started_at",
            "finished_at",
            "created_at",
        ],
        Table::VerificationCheckResult => &[
            "id",
            "run_id",
            "name",
            "kind",
            "required",
            "repository_id",
            "status",
            "exit_code",
            "duration_ms",
            "timed_out",
            "log_artifact_id",
            "output_excerpt",
            "failure_signatures",
            "created_at",
        ],
        Table::TaskAcceptance => &[
            "id",
            "task_id",
            "attempt_id",
            "verification_run_id",
            "decision",
            "mode",
            "reason",
            "actor",
            "manifest_id",
            "created_at",
        ],
        Table::Classification => &[
            "id",
            "task_id",
            "source",
            "status",
            "provider",
            "model",
            "prompt_version",
            "taxonomy_version",
            "input_hash",
            "category",
            "subtype",
            "domain",
            "affected_systems",
            "complexity",
            "tags",
            "confidence",
            "rationale",
            "raw_output",
            "validation_status",
            "error",
            "duration_ms",
            "is_current",
            "confirmed_by",
            "batch_id",
            "created_by",
            "created_at",
        ],
        Table::ClassificationGoldLabel => &[
            "id",
            "workspace_id",
            "task_id",
            "title",
            "description",
            "category",
            "labeled_by",
            "created_at",
        ],
        Table::ClassificationEvaluation => &[
            "id",
            "workspace_id",
            "provider",
            "model",
            "prompt_version",
            "taxonomy_version",
            "sample_size",
            "failures",
            "accuracy",
            "macro_f1",
            "per_class",
            "confusion",
            "results",
            "created_at",
        ],
        Table::TaskEmbedding => &[
            "id",
            "task_id",
            "provider",
            "model",
            "dimensions",
            "vector",
            "input_hash",
            "created_at",
        ],
        Table::AutomationRule => &[
            "id",
            "workspace_id",
            "name",
            "description",
            "enabled",
            "trigger",
            "conditions",
            "actions",
            "retry_max_attempts",
            "max_depth",
            "last_scheduled_at",
            "next_run_at",
            "created_by",
            "version",
            "created_at",
            "updated_at",
        ],
        Table::AutomationRun => &[
            "id",
            "rule_id",
            "workspace_id",
            "trigger_type",
            "trigger_key",
            "trigger_event_id",
            "depth",
            "dry_run",
            "status",
            "matched",
            "reason",
            "condition_trace",
            "action_results",
            "error",
            "attempts",
            "correlation_id",
            "created_at",
            "finished_at",
            "inputs",
        ],
        Table::Notification => &[
            "id",
            "workspace_id",
            "level",
            "title",
            "body",
            "source",
            "data",
            "read_at",
            "created_at",
        ],
        Table::Report => &[
            "id",
            "workspace_id",
            "kind",
            "title",
            "scope",
            "content_markdown",
            "data",
            "narrative_source",
            "created_by",
            "created_at",
        ],
        Table::Finding => &[
            "id",
            "workspace_id",
            "kind",
            "fingerprint",
            "title",
            "severity",
            "metric_value",
            "sample_size",
            "evidence",
            "first_detected_at",
            "last_detected_at",
        ],
        Table::AgentSession => &[
            "id",
            "provider",
            "external_session_id",
            "workspace_id",
            "attempt_id",
            "task_id",
            "binding_source",
            "cwd",
            "model",
            "transcript_path",
            "started_at",
            "ended_at",
            "end_reason",
            "tool_calls",
            "tool_failures",
            "created_at",
            "updated_at",
        ],
        Table::SessionBinding => &[
            "id",
            "workspace_id",
            "task_id",
            "provider",
            "external_session_id",
            "cwd_prefix",
            "active",
            "created_by",
            "created_at",
        ],
        Table::SubagentRun => &[
            "id",
            "session_id",
            "external_agent_id",
            "agent_type",
            "attempt_id",
            "started_at",
            "stopped_at",
            "last_message_excerpt",
            "transcript_path",
            "requested_model",
            "model",
            "usage",
            "telemetry",
            "task_id",
        ],
        Table::AgentInternalTask => &[
            "id",
            "session_id",
            "external_task_id",
            "subject",
            "description",
            "status",
            "task_id",
            "external_agent_id",
            "created_at",
            "completed_at",
        ],
        Table::AgentProfile => &[
            "id",
            "workspace_id",
            "name",
            "adapter",
            "model",
            "permissions",
            "enabled",
            "last_seen_at",
            "created_at",
        ],
        Table::HookEvent => &[
            "id",
            "dedupe_key",
            "provider",
            "hook_event_name",
            "external_session_id",
            "session_id",
            "attempt_id",
            "payload",
            "payload_purged_at",
            "received_at",
            "source",
        ],
        Table::ToolEvent => &[
            "id",
            "hook_event_id",
            "session_id",
            "attempt_id",
            "external_agent_id",
            "phase",
            "tool_name",
            "tool_use_id",
            "duration_ms",
            "error_excerpt",
            "summary",
            "occurred_at",
        ],
        Table::Experiment => &[
            "id",
            "workspace_id",
            "slug",
            "title",
            "question",
            "status",
            "protocol",
            "frozen_at",
            "created_at",
            "updated_at",
        ],
        Table::ExperimentTrial => &[
            "id",
            "experiment_id",
            "task_id",
            "attempt_id",
            "subagent_run_id",
            "arm",
            "requested_model",
            "actual_model",
            "phase",
            "seq",
            "band",
            "features",
            "base_ref",
            "worktree_path",
            "status",
            "outcome",
            "outcome_detail",
            "failure_cause",
            "failure_cause_source",
            "notes",
            "created_at",
            "started_at",
            "graded_at",
        ],
        Table::PredictorFit => &[
            "id",
            "workspace_id",
            "model",
            "created_at",
            "data_cutoff",
            "trial_ids",
            "feature_names",
            "repositories",
            "coef",
            "penalty",
            "n",
            "converged",
            "metrics",
            "samples",
        ],
        Table::AgentAttempt => &[
            "id",
            "workspace_id",
            "task_id",
            "number",
            "status",
            "provider",
            "adapter",
            "agent_id",
            "model",
            "session_id",
            "parent_attempt_id",
            "retry_of_attempt_id",
            "rerun_of_attempt_id",
            "created_by",
            "created_at",
            "queued_at",
            "started_at",
            "finished_at",
            "queue_wait_ms",
            "duration_ms",
            "exit_reason",
            "exit_code",
            "error",
            "summary",
            "input_task_version",
            "input_task_snapshot",
            "prompt_artifact_id",
            "config_snapshot",
            "environment",
            "baseline_manifest_id",
            "outcome_manifest_id",
            "usage",
            "usage_source",
            "cost_usd",
            "cost_source",
            "telemetry",
            "modified_repository_ids",
            "retries",
            "cancel_requested",
            "updated_at",
        ],
        Table::RevisionManifest => &[
            "id",
            "workspace_id",
            "attempt_id",
            "kind",
            "entries",
            "created_at",
        ],
        Table::ManagedWorktree => &[
            "id",
            "workspace_id",
            "repository_id",
            "source_checkout_id",
            "attempt_id",
            "path",
            "branch",
            "base_sha",
            "status",
            "last_error",
            "created_at",
            "updated_at",
            "removed_at",
        ],
        Table::ResourceLease => &[
            "resource_key",
            "holder",
            "purpose",
            "acquired_at",
            "expires_at",
        ],
        Table::ChangeSet => &[
            "id",
            "workspace_id",
            "task_id",
            "attempt_id",
            "title",
            "status",
            "created_at",
            "updated_at",
        ],
        Table::ChangeSetItem => &[
            "id",
            "change_set_id",
            "repository_id",
            "base_sha",
            "head_sha",
            "branch",
            "commit_shas",
            "status",
            "detail",
            "updated_at",
        ],
        Table::Artifact => &[
            "id",
            "workspace_id",
            "task_id",
            "attempt_id",
            "kind",
            "name",
            "media_type",
            "path",
            "size_bytes",
            "sha256",
            "retention",
            "purged_at",
            "metadata",
            "created_at",
        ],
        Table::Stack => &[
            "id",
            "workspace_id",
            "parent_task_id",
            "status",
            "created_at",
            "updated_at",
            "land_request",
        ],
        Table::StackRepository => &[
            "stack_id",
            "repository_id",
            "base_sha",
            "integration_branch",
            "created_at",
        ],
        Table::StackItem => &[
            "id",
            "stack_id",
            "task_id",
            "state",
            "merge_order",
            "detail",
            "created_at",
            "updated_at",
        ],
        Table::Job => &[
            "id",
            "workspace_id",
            "kind",
            "status",
            "priority",
            "payload",
            "result",
            "run_at",
            "attempts",
            "max_attempts",
            "backoff_base_seconds",
            "timeout_seconds",
            "lease_owner",
            "lease_expires_at",
            "heartbeat_at",
            "started_at",
            "finished_at",
            "cancel_requested",
            "idempotency_key",
            "resource_keys",
            "last_error",
            "error_kind",
            "correlation_id",
            "causation_id",
            "task_id",
            "attempt_id",
            "created_by",
            "created_at",
            "updated_at",
        ],
        Table::JobRun => &[
            "id",
            "job_id",
            "attempt_number",
            "worker_id",
            "status",
            "error",
            "started_at",
            "finished_at",
        ],
        Table::JobResourceClaim => &["job_id", "resource_key"],
        Table::Worker => &[
            "id",
            "hostname",
            "pid",
            "status",
            "concurrency",
            "kinds",
            "current_jobs",
            "jobs_completed",
            "jobs_failed",
            "started_at",
            "heartbeat_at",
            "stopped_at",
        ],
        Table::SystemCursor => &["name", "value", "updated_at"],
        Table::Workspace
        | Table::Project
        | Table::ProjectRepository
        | Table::Repository
        | Table::RepositoryCheckout
        | Table::RepositoryRemote
        | Table::RepositoryRef
        | Table::RepositoryDependency
        | Table::Task
        | Table::TaskFile
        | Table::TaskTag
        | Table::TaskLink
        | Table::TaskComment
        | Table::TaskDependency
        | Table::TaskRelation
        | Table::TaskProject
        | Table::TaskRepository
        | Table::ExternalItem => &[],
    }
}

/// Primary-key columns of the tables beyond the first 18 (`id` unless listed).
pub(crate) const fn primary_key_more(table: Table) -> &'static [&'static str] {
    match table {
        Table::CommitAnalysis => &["commit_id"],
        Table::ResourceLease => &["resource_key"],
        Table::StackRepository => &["stack_id", "repository_id"],
        Table::JobResourceClaim => &["job_id", "resource_key"],
        Table::SystemCursor => &["name"],
        _ => &["id"],
    }
}

/// Primary-key values of keys of the tables beyond the first 18; empty for those 18.
pub(crate) fn key_values_more(key: &RowKey) -> Vec<Raw> {
    match key {
        RowKey::Commit(id)
        | RowKey::CommitFileChange(id)
        | RowKey::CommitSymbol(id)
        | RowKey::CommitAnalysis(id)
        | RowKey::CommitEquivalence(id)
        | RowKey::CommitTaskLink(id)
        | RowKey::GitObjectGap(id)
        | RowKey::TaskChangeSurface(id)
        | RowKey::VerificationConfig(id)
        | RowKey::VerificationRun(id)
        | RowKey::VerificationCheckResult(id)
        | RowKey::TaskAcceptance(id)
        | RowKey::Classification(id)
        | RowKey::ClassificationGoldLabel(id)
        | RowKey::ClassificationEvaluation(id)
        | RowKey::TaskEmbedding(id)
        | RowKey::AutomationRule(id)
        | RowKey::AutomationRun(id)
        | RowKey::Notification(id)
        | RowKey::Report(id)
        | RowKey::Finding(id)
        | RowKey::AgentSession(id)
        | RowKey::SessionBinding(id)
        | RowKey::SubagentRun(id)
        | RowKey::AgentInternalTask(id)
        | RowKey::AgentProfile(id)
        | RowKey::HookEvent(id)
        | RowKey::ToolEvent(id)
        | RowKey::Experiment(id)
        | RowKey::ExperimentTrial(id)
        | RowKey::PredictorFit(id)
        | RowKey::AgentAttempt(id)
        | RowKey::RevisionManifest(id)
        | RowKey::ManagedWorktree(id)
        | RowKey::ChangeSet(id)
        | RowKey::ChangeSetItem(id)
        | RowKey::Artifact(id)
        | RowKey::Stack(id)
        | RowKey::StackItem(id)
        | RowKey::Job(id)
        | RowKey::JobRun(id) => vec![id_sql(*id)],
        RowKey::ResourceLease(text) | RowKey::Worker(text) | RowKey::SystemCursor(text) => {
            vec![text_sql(text)]
        }
        RowKey::StackRepository {
            stack_id,
            repository_id,
        } => vec![id_sql(*stack_id), id_sql(*repository_id)],
        RowKey::JobResourceClaim {
            job_id,
            resource_key,
        } => vec![id_sql(*job_id), text_sql(resource_key)],
        RowKey::Workspace(_)
        | RowKey::Project(_)
        | RowKey::ProjectRepository { .. }
        | RowKey::Repository(_)
        | RowKey::RepositoryCheckout(_)
        | RowKey::RepositoryRemote(_)
        | RowKey::RepositoryRef(_)
        | RowKey::RepositoryDependency(_)
        | RowKey::Task(_)
        | RowKey::TaskFile { .. }
        | RowKey::TaskTag { .. }
        | RowKey::TaskLink(_)
        | RowKey::TaskComment(_)
        | RowKey::TaskDependency(_)
        | RowKey::TaskRelation(_)
        | RowKey::TaskProject { .. }
        | RowKey::TaskRepository { .. }
        | RowKey::ExternalItem(_) => Vec::new(),
    }
}

/// Bound parameters of rows of the tables beyond the first 18; empty for those 18.
pub(crate) fn row_values_more(row: &Row) -> Result<Vec<Raw>, WgError> {
    match row {
        Row::Commit(row) => row.values(),
        Row::CommitFileChange(row) => row.values(),
        Row::CommitSymbol(row) => row.values(),
        Row::CommitAnalysis(row) => row.values(),
        Row::CommitEquivalence(row) => row.values(),
        Row::CommitTaskLink(row) => row.values(),
        Row::GitObjectGap(row) => row.values(),
        Row::TaskChangeSurface(row) => row.values(),
        Row::VerificationConfig(row) => row.values(),
        Row::VerificationRun(row) => row.values(),
        Row::VerificationCheckResult(row) => row.values(),
        Row::TaskAcceptance(row) => row.values(),
        Row::Classification(row) => row.values(),
        Row::ClassificationGoldLabel(row) => row.values(),
        Row::ClassificationEvaluation(row) => row.values(),
        Row::TaskEmbedding(row) => row.values(),
        Row::AutomationRule(row) => row.values(),
        Row::AutomationRun(row) => row.values(),
        Row::Notification(row) => row.values(),
        Row::Report(row) => row.values(),
        Row::Finding(row) => row.values(),
        Row::AgentSession(row) => row.values(),
        Row::SessionBinding(row) => row.values(),
        Row::SubagentRun(row) => row.values(),
        Row::AgentInternalTask(row) => row.values(),
        Row::AgentProfile(row) => row.values(),
        Row::HookEvent(row) => row.values(),
        Row::ToolEvent(row) => row.values(),
        Row::Experiment(row) => row.values(),
        Row::ExperimentTrial(row) => row.values(),
        Row::PredictorFit(row) => row.values(),
        Row::AgentAttempt(row) => row.values(),
        Row::RevisionManifest(row) => row.values(),
        Row::ManagedWorktree(row) => row.values(),
        Row::ResourceLease(row) => row.values(),
        Row::ChangeSet(row) => row.values(),
        Row::ChangeSetItem(row) => row.values(),
        Row::Artifact(row) => row.values(),
        Row::Stack(row) => row.values(),
        Row::StackRepository(row) => row.values(),
        Row::StackItem(row) => row.values(),
        Row::Job(row) => row.values(),
        Row::JobRun(row) => row.values(),
        Row::JobResourceClaim(row) => row.values(),
        Row::Worker(row) => row.values(),
        Row::SystemCursor(row) => row.values(),
        Row::Workspace(_)
        | Row::Project(_)
        | Row::ProjectRepository(_)
        | Row::Repository(_)
        | Row::RepositoryCheckout(_)
        | Row::RepositoryRemote(_)
        | Row::RepositoryRef(_)
        | Row::RepositoryDependency(_)
        | Row::Task(_)
        | Row::TaskFile(_)
        | Row::TaskTag(_)
        | Row::TaskLink(_)
        | Row::TaskComment(_)
        | Row::TaskDependency(_)
        | Row::TaskRelation(_)
        | Row::TaskProject(_)
        | Row::TaskRepository(_)
        | Row::ExternalItem(_) => Ok(Vec::new()),
    }
}

impl TableRow for Commit {
    const TABLE: Table = Table::Commit;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.repository_id),
            text_sql(&self.sha),
            opt_text_sql(&self.tree_sha),
            strings_sql(&self.parents)?,
            text_sql(&self.author_name),
            text_sql(&self.author_email),
            opt_ts_sql(self.authored_at),
            text_sql(&self.committer_name),
            text_sql(&self.committer_email),
            opt_ts_sql(self.committed_at),
            text_sql(&self.subject),
            text_sql(&self.message),
            objects_sql(&self.trailers)?,
            bool_sql(self.is_merge),
            opt_text_sql(&self.patch_id),
            Raw::Int(self.insertions),
            Raw::Int(self.deletions),
            Raw::Int(self.files_changed),
            bool_sql(self.reachable),
            opt_ts_sql(self.unreachable_since),
            bool_sql(self.object_available),
            bool_sql(self.structure_analyzed),
            strings_sql(&self.branches)?,
            ts_sql(self.ingested_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Commit {
            id: c.id("id")?,
            repository_id: c.id("repository_id")?,
            sha: c.text("sha")?,
            tree_sha: c.opt_text("tree_sha")?,
            parents: c.strings("parents")?,
            author_name: c.text("author_name")?,
            author_email: c.text("author_email")?,
            authored_at: c.opt_ts("authored_at")?,
            committer_name: c.text("committer_name")?,
            committer_email: c.text("committer_email")?,
            committed_at: c.opt_ts("committed_at")?,
            subject: c.text("subject")?,
            message: c.text("message")?,
            trailers: c.objects("trailers")?,
            is_merge: c.flag("is_merge")?,
            patch_id: c.opt_text("patch_id")?,
            insertions: c.int("insertions")?,
            deletions: c.int("deletions")?,
            files_changed: c.int("files_changed")?,
            reachable: c.flag("reachable")?,
            unreachable_since: c.opt_ts("unreachable_since")?,
            object_available: c.flag("object_available")?,
            structure_analyzed: c.flag("structure_analyzed")?,
            branches: c.strings("branches")?,
            ingested_at: c.ts("ingested_at")?,
        })
    }
}

impl TableRow for CommitFileChange {
    const TABLE: Table = Table::CommitFileChange;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.commit_id),
            text_sql(&self.path),
            opt_text_sql(&self.old_path),
            text_sql(&self.change_type),
            opt_int_sql(self.similarity),
            Raw::Int(self.insertions),
            Raw::Int(self.deletions),
            bool_sql(self.is_binary),
            opt_text_sql(&self.language),
            text_sql(&self.file_class),
            text_sql(&self.module),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(CommitFileChange {
            id: c.id("id")?,
            commit_id: c.id("commit_id")?,
            path: c.text("path")?,
            old_path: c.opt_text("old_path")?,
            change_type: c.text("change_type")?,
            similarity: c.opt_int("similarity")?,
            insertions: c.int("insertions")?,
            deletions: c.int("deletions")?,
            is_binary: c.flag("is_binary")?,
            language: c.opt_text("language")?,
            file_class: c.text("file_class")?,
            module: c.text("module")?,
        })
    }
}

impl TableRow for CommitSymbol {
    const TABLE: Table = Table::CommitSymbol;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.file_change_id),
            text_sql(&self.symbol_kind),
            text_sql(&self.name),
            text_sql(&self.change_kind),
            opt_int_sql(self.start_line),
            opt_int_sql(self.end_line),
            Raw::Int(self.lines_changed),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(CommitSymbol {
            id: c.id("id")?,
            file_change_id: c.id("file_change_id")?,
            symbol_kind: c.text("symbol_kind")?,
            name: c.text("name")?,
            change_kind: c.text("change_kind")?,
            start_line: c.opt_int("start_line")?,
            end_line: c.opt_int("end_line")?,
            lines_changed: c.int("lines_changed")?,
        })
    }
}

impl TableRow for CommitAnalysis {
    const TABLE: Table = Table::CommitAnalysis;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.commit_id),
            text_sql(&self.analyzer_version),
            Raw::Int(self.files_parsed),
            Raw::Int(self.files_fallback),
            Raw::Int(self.functions_affected),
            Raw::Int(self.types_affected),
            object_sql(&self.detail)?,
            ts_sql(self.analyzed_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(CommitAnalysis {
            commit_id: c.id("commit_id")?,
            analyzer_version: c.text("analyzer_version")?,
            files_parsed: c.int("files_parsed")?,
            files_fallback: c.int("files_fallback")?,
            functions_affected: c.int("functions_affected")?,
            types_affected: c.int("types_affected")?,
            detail: c.object("detail")?,
            analyzed_at: c.ts("analyzed_at")?,
        })
    }
}

impl TableRow for CommitEquivalence {
    const TABLE: Table = Table::CommitEquivalence;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.repository_id),
            id_sql(self.original_commit_id),
            id_sql(self.equivalent_commit_id),
            text_sql(&self.kind),
            object_sql(&self.evidence)?,
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(CommitEquivalence {
            id: c.id("id")?,
            repository_id: c.id("repository_id")?,
            original_commit_id: c.id("original_commit_id")?,
            equivalent_commit_id: c.id("equivalent_commit_id")?,
            kind: c.text("kind")?,
            evidence: c.object("evidence")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for CommitTaskLink {
    const TABLE: Table = Table::CommitTaskLink;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.commit_id),
            id_sql(self.task_id),
            text_sql(&self.source),
            real_sql(self.confidence),
            object_sql(&self.evidence)?,
            opt_id_sql(self.attempt_id),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(CommitTaskLink {
            id: c.id("id")?,
            commit_id: c.id("commit_id")?,
            task_id: c.id("task_id")?,
            source: c.text("source")?,
            confidence: c.real("confidence")?,
            evidence: c.object("evidence")?,
            attempt_id: c.opt_id("attempt_id")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for GitObjectGap {
    const TABLE: Table = Table::GitObjectGap;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.repository_id),
            text_sql(&self.sha),
            text_sql(&self.context),
            ts_sql(self.detected_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(GitObjectGap {
            id: c.id("id")?,
            repository_id: c.id("repository_id")?,
            sha: c.text("sha")?,
            context: c.text("context")?,
            detected_at: c.ts("detected_at")?,
        })
    }
}

impl TableRow for TaskChangeSurface {
    const TABLE: Table = Table::TaskChangeSurface;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.task_id),
            id_sql(self.repository_id),
            opt_id_sql(self.attempt_id),
            text_sql(&self.base_sha),
            text_sql(&self.final_sha),
            Raw::Int(self.files_changed),
            Raw::Int(self.insertions),
            Raw::Int(self.deletions),
            Raw::Int(self.renames),
            Raw::Int(self.binary_files),
            Raw::Int(self.functions_affected),
            Raw::Int(self.types_affected),
            object_sql(&self.detail)?,
            ts_sql(self.computed_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(TaskChangeSurface {
            id: c.id("id")?,
            task_id: c.id("task_id")?,
            repository_id: c.id("repository_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            base_sha: c.text("base_sha")?,
            final_sha: c.text("final_sha")?,
            files_changed: c.int("files_changed")?,
            insertions: c.int("insertions")?,
            deletions: c.int("deletions")?,
            renames: c.int("renames")?,
            binary_files: c.int("binary_files")?,
            functions_affected: c.int("functions_affected")?,
            types_affected: c.int("types_affected")?,
            detail: c.object("detail")?,
            computed_at: c.ts("computed_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for VerificationConfig {
    const TABLE: Table = Table::VerificationConfig;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.scope_type),
            id_sql(self.scope_id),
            text_sql(&self.name),
            objects_sql(&self.checks)?,
            bool_sql(self.enabled),
            text_sql(&self.approved_by),
            Raw::Int(self.version),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(VerificationConfig {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            scope_type: c.text("scope_type")?,
            scope_id: c.id("scope_id")?,
            name: c.text("name")?,
            checks: c.objects("checks")?,
            enabled: c.flag("enabled")?,
            approved_by: c.text("approved_by")?,
            version: c.int("version")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for VerificationRun {
    const TABLE: Table = Table::VerificationRun;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            id_sql(self.task_id),
            opt_id_sql(self.attempt_id),
            opt_id_sql(self.manifest_id),
            objects_sql(&self.config_snapshot)?,
            text_sql(&self.status),
            bool_sql(self.required_passed),
            text_sql(&self.triggered_by),
            opt_ts_sql(self.started_at),
            opt_ts_sql(self.finished_at),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(VerificationRun {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            task_id: c.id("task_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            manifest_id: c.opt_id("manifest_id")?,
            config_snapshot: c.objects("config_snapshot")?,
            status: c.text("status")?,
            required_passed: c.flag("required_passed")?,
            triggered_by: c.text("triggered_by")?,
            started_at: c.opt_ts("started_at")?,
            finished_at: c.opt_ts("finished_at")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for VerificationCheckResult {
    const TABLE: Table = Table::VerificationCheckResult;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.run_id),
            text_sql(&self.name),
            text_sql(&self.kind),
            bool_sql(self.required),
            opt_id_sql(self.repository_id),
            text_sql(&self.status),
            opt_int_sql(self.exit_code),
            opt_int_sql(self.duration_ms),
            bool_sql(self.timed_out),
            opt_id_sql(self.log_artifact_id),
            text_sql(&self.output_excerpt),
            strings_sql(&self.failure_signatures)?,
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(VerificationCheckResult {
            id: c.id("id")?,
            run_id: c.id("run_id")?,
            name: c.text("name")?,
            kind: c.text("kind")?,
            required: c.flag("required")?,
            repository_id: c.opt_id("repository_id")?,
            status: c.text("status")?,
            exit_code: c.opt_int("exit_code")?,
            duration_ms: c.opt_int("duration_ms")?,
            timed_out: c.flag("timed_out")?,
            log_artifact_id: c.opt_id("log_artifact_id")?,
            output_excerpt: c.text("output_excerpt")?,
            failure_signatures: c.strings("failure_signatures")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for TaskAcceptance {
    const TABLE: Table = Table::TaskAcceptance;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.task_id),
            opt_id_sql(self.attempt_id),
            opt_id_sql(self.verification_run_id),
            text_sql(&self.decision),
            text_sql(&self.mode),
            text_sql(&self.reason),
            text_sql(&self.actor),
            opt_id_sql(self.manifest_id),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(TaskAcceptance {
            id: c.id("id")?,
            task_id: c.id("task_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            verification_run_id: c.opt_id("verification_run_id")?,
            decision: c.text("decision")?,
            mode: c.text("mode")?,
            reason: c.text("reason")?,
            actor: c.text("actor")?,
            manifest_id: c.opt_id("manifest_id")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for Classification {
    const TABLE: Table = Table::Classification;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.task_id),
            text_sql(&self.source),
            text_sql(&self.status),
            opt_text_sql(&self.provider),
            opt_text_sql(&self.model),
            opt_text_sql(&self.prompt_version),
            text_sql(&self.taxonomy_version),
            text_sql(&self.input_hash),
            opt_text_sql(&self.category),
            opt_text_sql(&self.subtype),
            opt_text_sql(&self.domain),
            strings_sql(&self.affected_systems)?,
            opt_text_sql(&self.complexity),
            strings_sql(&self.tags)?,
            opt_real_sql(self.confidence),
            opt_text_sql(&self.rationale),
            opt_object_sql(&self.raw_output)?,
            text_sql(&self.validation_status),
            opt_text_sql(&self.error),
            opt_int_sql(self.duration_ms),
            bool_sql(self.is_current),
            opt_text_sql(&self.confirmed_by),
            opt_id_sql(self.batch_id),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Classification {
            id: c.id("id")?,
            task_id: c.id("task_id")?,
            source: c.text("source")?,
            status: c.text("status")?,
            provider: c.opt_text("provider")?,
            model: c.opt_text("model")?,
            prompt_version: c.opt_text("prompt_version")?,
            taxonomy_version: c.text("taxonomy_version")?,
            input_hash: c.text("input_hash")?,
            category: c.opt_text("category")?,
            subtype: c.opt_text("subtype")?,
            domain: c.opt_text("domain")?,
            affected_systems: c.strings("affected_systems")?,
            complexity: c.opt_text("complexity")?,
            tags: c.strings("tags")?,
            confidence: c.opt_real("confidence")?,
            rationale: c.opt_text("rationale")?,
            raw_output: c.opt_object("raw_output")?,
            validation_status: c.text("validation_status")?,
            error: c.opt_text("error")?,
            duration_ms: c.opt_int("duration_ms")?,
            is_current: c.flag("is_current")?,
            confirmed_by: c.opt_text("confirmed_by")?,
            batch_id: c.opt_id("batch_id")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for ClassificationGoldLabel {
    const TABLE: Table = Table::ClassificationGoldLabel;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            opt_id_sql(self.task_id),
            text_sql(&self.title),
            text_sql(&self.description),
            text_sql(&self.category),
            text_sql(&self.labeled_by),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(ClassificationGoldLabel {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            task_id: c.opt_id("task_id")?,
            title: c.text("title")?,
            description: c.text("description")?,
            category: c.text("category")?,
            labeled_by: c.text("labeled_by")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for ClassificationEvaluation {
    const TABLE: Table = Table::ClassificationEvaluation;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.provider),
            text_sql(&self.model),
            text_sql(&self.prompt_version),
            text_sql(&self.taxonomy_version),
            Raw::Int(self.sample_size),
            Raw::Int(self.failures),
            opt_real_sql(self.accuracy),
            opt_real_sql(self.macro_f1),
            object_sql(&self.per_class)?,
            object_sql(&self.confusion)?,
            objects_sql(&self.results)?,
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(ClassificationEvaluation {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            provider: c.text("provider")?,
            model: c.text("model")?,
            prompt_version: c.text("prompt_version")?,
            taxonomy_version: c.text("taxonomy_version")?,
            sample_size: c.int("sample_size")?,
            failures: c.int("failures")?,
            accuracy: c.opt_real("accuracy")?,
            macro_f1: c.opt_real("macro_f1")?,
            per_class: c.object("per_class")?,
            confusion: c.object("confusion")?,
            results: c.objects("results")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for TaskEmbedding {
    const TABLE: Table = Table::TaskEmbedding;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.task_id),
            text_sql(&self.provider),
            text_sql(&self.model),
            Raw::Int(self.dimensions),
            blob_sql(&self.vector),
            text_sql(&self.input_hash),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(TaskEmbedding {
            id: c.id("id")?,
            task_id: c.id("task_id")?,
            provider: c.text("provider")?,
            model: c.text("model")?,
            dimensions: c.int("dimensions")?,
            vector: c.blob("vector")?,
            input_hash: c.text("input_hash")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for AutomationRule {
    const TABLE: Table = Table::AutomationRule;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.name),
            text_sql(&self.description),
            bool_sql(self.enabled),
            object_sql(&self.trigger)?,
            object_sql(&self.conditions)?,
            objects_sql(&self.actions)?,
            Raw::Int(self.retry_max_attempts),
            Raw::Int(self.max_depth),
            opt_ts_sql(self.last_scheduled_at),
            opt_ts_sql(self.next_run_at),
            text_sql(&self.created_by),
            Raw::Int(self.version),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(AutomationRule {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            name: c.text("name")?,
            description: c.text("description")?,
            enabled: c.flag("enabled")?,
            trigger: c.object("trigger")?,
            conditions: c.object("conditions")?,
            actions: c.objects("actions")?,
            retry_max_attempts: c.int("retry_max_attempts")?,
            max_depth: c.int("max_depth")?,
            last_scheduled_at: c.opt_ts("last_scheduled_at")?,
            next_run_at: c.opt_ts("next_run_at")?,
            created_by: c.text("created_by")?,
            version: c.int("version")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for AutomationRun {
    const TABLE: Table = Table::AutomationRun;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.rule_id),
            id_sql(self.workspace_id),
            text_sql(&self.trigger_type),
            text_sql(&self.trigger_key),
            opt_id_sql(self.trigger_event_id),
            Raw::Int(self.depth),
            bool_sql(self.dry_run),
            text_sql(&self.status),
            bool_sql(self.matched),
            text_sql(&self.reason),
            objects_sql(&self.condition_trace)?,
            objects_sql(&self.action_results)?,
            opt_text_sql(&self.error),
            Raw::Int(self.attempts),
            opt_id_sql(self.correlation_id),
            ts_sql(self.created_at),
            opt_ts_sql(self.finished_at),
            object_sql(&self.inputs)?,
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(AutomationRun {
            id: c.id("id")?,
            rule_id: c.id("rule_id")?,
            workspace_id: c.id("workspace_id")?,
            trigger_type: c.text("trigger_type")?,
            trigger_key: c.text("trigger_key")?,
            trigger_event_id: c.opt_id("trigger_event_id")?,
            depth: c.int("depth")?,
            dry_run: c.flag("dry_run")?,
            status: c.text("status")?,
            matched: c.flag("matched")?,
            reason: c.text("reason")?,
            condition_trace: c.objects("condition_trace")?,
            action_results: c.objects("action_results")?,
            error: c.opt_text("error")?,
            attempts: c.int("attempts")?,
            correlation_id: c.opt_id("correlation_id")?,
            created_at: c.ts("created_at")?,
            finished_at: c.opt_ts("finished_at")?,
            inputs: c.object("inputs")?,
        })
    }
}

impl TableRow for Notification {
    const TABLE: Table = Table::Notification;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.level),
            text_sql(&self.title),
            text_sql(&self.body),
            text_sql(&self.source),
            object_sql(&self.data)?,
            opt_ts_sql(self.read_at),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Notification {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            level: c.text("level")?,
            title: c.text("title")?,
            body: c.text("body")?,
            source: c.text("source")?,
            data: c.object("data")?,
            read_at: c.opt_ts("read_at")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for Report {
    const TABLE: Table = Table::Report;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.kind),
            text_sql(&self.title),
            object_sql(&self.scope)?,
            text_sql(&self.content_markdown),
            object_sql(&self.data)?,
            text_sql(&self.narrative_source),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Report {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            kind: c.text("kind")?,
            title: c.text("title")?,
            scope: c.object("scope")?,
            content_markdown: c.text("content_markdown")?,
            data: c.object("data")?,
            narrative_source: c.text("narrative_source")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for Finding {
    const TABLE: Table = Table::Finding;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.kind),
            text_sql(&self.fingerprint),
            text_sql(&self.title),
            text_sql(&self.severity),
            opt_real_sql(self.metric_value),
            Raw::Int(self.sample_size),
            object_sql(&self.evidence)?,
            ts_sql(self.first_detected_at),
            ts_sql(self.last_detected_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Finding {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            kind: c.text("kind")?,
            fingerprint: c.text("fingerprint")?,
            title: c.text("title")?,
            severity: c.text("severity")?,
            metric_value: c.opt_real("metric_value")?,
            sample_size: c.int("sample_size")?,
            evidence: c.object("evidence")?,
            first_detected_at: c.ts("first_detected_at")?,
            last_detected_at: c.ts("last_detected_at")?,
        })
    }
}

impl TableRow for AgentSession {
    const TABLE: Table = Table::AgentSession;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            text_sql(&self.provider),
            text_sql(&self.external_session_id),
            opt_id_sql(self.workspace_id),
            opt_id_sql(self.attempt_id),
            opt_id_sql(self.task_id),
            text_sql(&self.binding_source),
            opt_text_sql(&self.cwd),
            opt_text_sql(&self.model),
            opt_text_sql(&self.transcript_path),
            opt_ts_sql(self.started_at),
            opt_ts_sql(self.ended_at),
            opt_text_sql(&self.end_reason),
            Raw::Int(self.tool_calls),
            Raw::Int(self.tool_failures),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(AgentSession {
            id: c.id("id")?,
            provider: c.text("provider")?,
            external_session_id: c.text("external_session_id")?,
            workspace_id: c.opt_id("workspace_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            task_id: c.opt_id("task_id")?,
            binding_source: c.text("binding_source")?,
            cwd: c.opt_text("cwd")?,
            model: c.opt_text("model")?,
            transcript_path: c.opt_text("transcript_path")?,
            started_at: c.opt_ts("started_at")?,
            ended_at: c.opt_ts("ended_at")?,
            end_reason: c.opt_text("end_reason")?,
            tool_calls: c.int("tool_calls")?,
            tool_failures: c.int("tool_failures")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for SessionBinding {
    const TABLE: Table = Table::SessionBinding;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            id_sql(self.task_id),
            text_sql(&self.provider),
            opt_text_sql(&self.external_session_id),
            opt_text_sql(&self.cwd_prefix),
            bool_sql(self.active),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(SessionBinding {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            task_id: c.id("task_id")?,
            provider: c.text("provider")?,
            external_session_id: c.opt_text("external_session_id")?,
            cwd_prefix: c.opt_text("cwd_prefix")?,
            active: c.flag("active")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for SubagentRun {
    const TABLE: Table = Table::SubagentRun;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.session_id),
            text_sql(&self.external_agent_id),
            opt_text_sql(&self.agent_type),
            opt_id_sql(self.attempt_id),
            opt_ts_sql(self.started_at),
            opt_ts_sql(self.stopped_at),
            opt_text_sql(&self.last_message_excerpt),
            opt_text_sql(&self.transcript_path),
            opt_text_sql(&self.requested_model),
            opt_text_sql(&self.model),
            opt_object_sql(&self.usage)?,
            opt_object_sql(&self.telemetry)?,
            opt_id_sql(self.task_id),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(SubagentRun {
            id: c.id("id")?,
            session_id: c.id("session_id")?,
            external_agent_id: c.text("external_agent_id")?,
            agent_type: c.opt_text("agent_type")?,
            attempt_id: c.opt_id("attempt_id")?,
            started_at: c.opt_ts("started_at")?,
            stopped_at: c.opt_ts("stopped_at")?,
            last_message_excerpt: c.opt_text("last_message_excerpt")?,
            transcript_path: c.opt_text("transcript_path")?,
            requested_model: c.opt_text("requested_model")?,
            model: c.opt_text("model")?,
            usage: c.opt_object("usage")?,
            telemetry: c.opt_object("telemetry")?,
            task_id: c.opt_id("task_id")?,
        })
    }
}

impl TableRow for AgentInternalTask {
    const TABLE: Table = Table::AgentInternalTask;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.session_id),
            text_sql(&self.external_task_id),
            text_sql(&self.subject),
            opt_text_sql(&self.description),
            text_sql(&self.status),
            opt_id_sql(self.task_id),
            opt_text_sql(&self.external_agent_id),
            ts_sql(self.created_at),
            opt_ts_sql(self.completed_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(AgentInternalTask {
            id: c.id("id")?,
            session_id: c.id("session_id")?,
            external_task_id: c.text("external_task_id")?,
            subject: c.text("subject")?,
            description: c.opt_text("description")?,
            status: c.text("status")?,
            task_id: c.opt_id("task_id")?,
            external_agent_id: c.opt_text("external_agent_id")?,
            created_at: c.ts("created_at")?,
            completed_at: c.opt_ts("completed_at")?,
        })
    }
}

impl TableRow for AgentProfile {
    const TABLE: Table = Table::AgentProfile;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.name),
            opt_text_sql(&self.adapter),
            opt_text_sql(&self.model),
            strings_sql(&self.permissions)?,
            bool_sql(self.enabled),
            opt_ts_sql(self.last_seen_at),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(AgentProfile {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            name: c.text("name")?,
            adapter: c.opt_text("adapter")?,
            model: c.opt_text("model")?,
            permissions: c.strings("permissions")?,
            enabled: c.flag("enabled")?,
            last_seen_at: c.opt_ts("last_seen_at")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for HookEvent {
    const TABLE: Table = Table::HookEvent;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            text_sql(&self.dedupe_key),
            text_sql(&self.provider),
            text_sql(&self.hook_event_name),
            opt_text_sql(&self.external_session_id),
            opt_id_sql(self.session_id),
            opt_id_sql(self.attempt_id),
            opt_object_sql(&self.payload)?,
            opt_ts_sql(self.payload_purged_at),
            ts_sql(self.received_at),
            text_sql(&self.source),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(HookEvent {
            id: c.id("id")?,
            dedupe_key: c.text("dedupe_key")?,
            provider: c.text("provider")?,
            hook_event_name: c.text("hook_event_name")?,
            external_session_id: c.opt_text("external_session_id")?,
            session_id: c.opt_id("session_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            payload: c.opt_object("payload")?,
            payload_purged_at: c.opt_ts("payload_purged_at")?,
            received_at: c.ts("received_at")?,
            source: c.text("source")?,
        })
    }
}

impl TableRow for ToolEvent {
    const TABLE: Table = Table::ToolEvent;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.hook_event_id),
            opt_id_sql(self.session_id),
            opt_id_sql(self.attempt_id),
            opt_text_sql(&self.external_agent_id),
            text_sql(&self.phase),
            text_sql(&self.tool_name),
            opt_text_sql(&self.tool_use_id),
            opt_int_sql(self.duration_ms),
            opt_text_sql(&self.error_excerpt),
            object_sql(&self.summary)?,
            ts_sql(self.occurred_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(ToolEvent {
            id: c.id("id")?,
            hook_event_id: c.id("hook_event_id")?,
            session_id: c.opt_id("session_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            external_agent_id: c.opt_text("external_agent_id")?,
            phase: c.text("phase")?,
            tool_name: c.text("tool_name")?,
            tool_use_id: c.opt_text("tool_use_id")?,
            duration_ms: c.opt_int("duration_ms")?,
            error_excerpt: c.opt_text("error_excerpt")?,
            summary: c.object("summary")?,
            occurred_at: c.ts("occurred_at")?,
        })
    }
}

impl TableRow for Experiment {
    const TABLE: Table = Table::Experiment;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.slug),
            text_sql(&self.title),
            opt_text_sql(&self.question),
            text_sql(&self.status),
            object_sql(&self.protocol)?,
            opt_ts_sql(self.frozen_at),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Experiment {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            slug: c.text("slug")?,
            title: c.text("title")?,
            question: c.opt_text("question")?,
            status: c.text("status")?,
            protocol: c.object("protocol")?,
            frozen_at: c.opt_ts("frozen_at")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for ExperimentTrial {
    const TABLE: Table = Table::ExperimentTrial;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.experiment_id),
            id_sql(self.task_id),
            opt_id_sql(self.attempt_id),
            opt_id_sql(self.subagent_run_id),
            text_sql(&self.arm),
            text_sql(&self.requested_model),
            opt_text_sql(&self.actual_model),
            text_sql(&self.phase),
            Raw::Int(self.seq),
            opt_int_sql(self.band),
            object_sql(&self.features)?,
            opt_text_sql(&self.base_ref),
            opt_text_sql(&self.worktree_path),
            text_sql(&self.status),
            opt_text_sql(&self.outcome),
            object_sql(&self.outcome_detail)?,
            opt_text_sql(&self.failure_cause),
            opt_text_sql(&self.failure_cause_source),
            opt_text_sql(&self.notes),
            ts_sql(self.created_at),
            opt_ts_sql(self.started_at),
            opt_ts_sql(self.graded_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(ExperimentTrial {
            id: c.id("id")?,
            experiment_id: c.id("experiment_id")?,
            task_id: c.id("task_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            subagent_run_id: c.opt_id("subagent_run_id")?,
            arm: c.text("arm")?,
            requested_model: c.text("requested_model")?,
            actual_model: c.opt_text("actual_model")?,
            phase: c.text("phase")?,
            seq: c.int("seq")?,
            band: c.opt_int("band")?,
            features: c.object("features")?,
            base_ref: c.opt_text("base_ref")?,
            worktree_path: c.opt_text("worktree_path")?,
            status: c.text("status")?,
            outcome: c.opt_text("outcome")?,
            outcome_detail: c.object("outcome_detail")?,
            failure_cause: c.opt_text("failure_cause")?,
            failure_cause_source: c.opt_text("failure_cause_source")?,
            notes: c.opt_text("notes")?,
            created_at: c.ts("created_at")?,
            started_at: c.opt_ts("started_at")?,
            graded_at: c.opt_ts("graded_at")?,
        })
    }
}

impl TableRow for PredictorFit {
    const TABLE: Table = Table::PredictorFit;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            text_sql(&self.model),
            ts_sql(self.created_at),
            opt_ts_sql(self.data_cutoff),
            strings_sql(&self.trial_ids)?,
            strings_sql(&self.feature_names)?,
            strings_sql(&self.repositories)?,
            values_sql(&self.coef)?,
            values_sql(&self.penalty)?,
            Raw::Int(self.n),
            bool_sql(self.converged),
            object_sql(&self.metrics)?,
            values_sql(&self.samples)?,
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(PredictorFit {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            model: c.text("model")?,
            created_at: c.ts("created_at")?,
            data_cutoff: c.opt_ts("data_cutoff")?,
            trial_ids: c.strings("trial_ids")?,
            feature_names: c.strings("feature_names")?,
            repositories: c.strings("repositories")?,
            coef: c.values("coef")?,
            penalty: c.values("penalty")?,
            n: c.int("n")?,
            converged: c.flag("converged")?,
            metrics: c.object("metrics")?,
            samples: c.values("samples")?,
        })
    }
}

impl TableRow for AgentAttempt {
    const TABLE: Table = Table::AgentAttempt;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            id_sql(self.task_id),
            Raw::Int(self.number),
            text_sql(self.status.as_str()),
            text_sql(&self.provider),
            text_sql(&self.adapter),
            opt_text_sql(&self.agent_id),
            opt_text_sql(&self.model),
            opt_text_sql(&self.session_id),
            opt_id_sql(self.parent_attempt_id),
            opt_id_sql(self.retry_of_attempt_id),
            opt_id_sql(self.rerun_of_attempt_id),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
            opt_ts_sql(self.queued_at),
            opt_ts_sql(self.started_at),
            opt_ts_sql(self.finished_at),
            opt_int_sql(self.queue_wait_ms),
            opt_int_sql(self.duration_ms),
            opt_text_sql(&self.exit_reason),
            opt_int_sql(self.exit_code),
            opt_text_sql(&self.error),
            opt_text_sql(&self.summary),
            opt_int_sql(self.input_task_version),
            object_sql(&self.input_task_snapshot)?,
            opt_id_sql(self.prompt_artifact_id),
            object_sql(&self.config_snapshot)?,
            object_sql(&self.environment)?,
            opt_id_sql(self.baseline_manifest_id),
            opt_id_sql(self.outcome_manifest_id),
            object_sql(&self.usage)?,
            opt_text_sql(&self.usage_source),
            opt_real_sql(self.cost_usd),
            opt_text_sql(&self.cost_source),
            object_sql(&self.telemetry)?,
            strings_sql(&self.modified_repository_ids)?,
            Raw::Int(self.retries),
            bool_sql(self.cancel_requested),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(AgentAttempt {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            task_id: c.id("task_id")?,
            number: c.int("number")?,
            status: c.parsed("status")?,
            provider: c.text("provider")?,
            adapter: c.text("adapter")?,
            agent_id: c.opt_text("agent_id")?,
            model: c.opt_text("model")?,
            session_id: c.opt_text("session_id")?,
            parent_attempt_id: c.opt_id("parent_attempt_id")?,
            retry_of_attempt_id: c.opt_id("retry_of_attempt_id")?,
            rerun_of_attempt_id: c.opt_id("rerun_of_attempt_id")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
            queued_at: c.opt_ts("queued_at")?,
            started_at: c.opt_ts("started_at")?,
            finished_at: c.opt_ts("finished_at")?,
            queue_wait_ms: c.opt_int("queue_wait_ms")?,
            duration_ms: c.opt_int("duration_ms")?,
            exit_reason: c.opt_text("exit_reason")?,
            exit_code: c.opt_int("exit_code")?,
            error: c.opt_text("error")?,
            summary: c.opt_text("summary")?,
            input_task_version: c.opt_int("input_task_version")?,
            input_task_snapshot: c.object("input_task_snapshot")?,
            prompt_artifact_id: c.opt_id("prompt_artifact_id")?,
            config_snapshot: c.object("config_snapshot")?,
            environment: c.object("environment")?,
            baseline_manifest_id: c.opt_id("baseline_manifest_id")?,
            outcome_manifest_id: c.opt_id("outcome_manifest_id")?,
            usage: c.object("usage")?,
            usage_source: c.opt_text("usage_source")?,
            cost_usd: c.opt_real("cost_usd")?,
            cost_source: c.opt_text("cost_source")?,
            telemetry: c.object("telemetry")?,
            modified_repository_ids: c.strings("modified_repository_ids")?,
            retries: c.int("retries")?,
            cancel_requested: c.flag("cancel_requested")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for RevisionManifest {
    const TABLE: Table = Table::RevisionManifest;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            opt_id_sql(self.attempt_id),
            text_sql(&self.kind),
            objects_sql(&self.entries)?,
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(RevisionManifest {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            kind: c.text("kind")?,
            entries: c.objects("entries")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for ManagedWorktree {
    const TABLE: Table = Table::ManagedWorktree;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            id_sql(self.repository_id),
            id_sql(self.source_checkout_id),
            opt_id_sql(self.attempt_id),
            text_sql(&self.path),
            text_sql(&self.branch),
            text_sql(&self.base_sha),
            text_sql(self.status.as_str()),
            opt_text_sql(&self.last_error),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
            opt_ts_sql(self.removed_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(ManagedWorktree {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            repository_id: c.id("repository_id")?,
            source_checkout_id: c.id("source_checkout_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            path: c.text("path")?,
            branch: c.text("branch")?,
            base_sha: c.text("base_sha")?,
            status: c.parsed("status")?,
            last_error: c.opt_text("last_error")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
            removed_at: c.opt_ts("removed_at")?,
        })
    }
}

impl TableRow for ResourceLease {
    const TABLE: Table = Table::ResourceLease;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            text_sql(&self.resource_key),
            text_sql(&self.holder),
            text_sql(&self.purpose),
            ts_sql(self.acquired_at),
            opt_ts_sql(self.expires_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(ResourceLease {
            resource_key: c.text("resource_key")?,
            holder: c.text("holder")?,
            purpose: c.text("purpose")?,
            acquired_at: c.ts("acquired_at")?,
            expires_at: c.opt_ts("expires_at")?,
        })
    }
}

impl TableRow for ChangeSet {
    const TABLE: Table = Table::ChangeSet;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            id_sql(self.task_id),
            opt_id_sql(self.attempt_id),
            text_sql(&self.title),
            text_sql(self.status.as_str()),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(ChangeSet {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            task_id: c.id("task_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            title: c.text("title")?,
            status: c.parsed("status")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for ChangeSetItem {
    const TABLE: Table = Table::ChangeSetItem;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.change_set_id),
            id_sql(self.repository_id),
            opt_text_sql(&self.base_sha),
            opt_text_sql(&self.head_sha),
            opt_text_sql(&self.branch),
            strings_sql(&self.commit_shas)?,
            text_sql(self.status.as_str()),
            opt_text_sql(&self.detail),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(ChangeSetItem {
            id: c.id("id")?,
            change_set_id: c.id("change_set_id")?,
            repository_id: c.id("repository_id")?,
            base_sha: c.opt_text("base_sha")?,
            head_sha: c.opt_text("head_sha")?,
            branch: c.opt_text("branch")?,
            commit_shas: c.strings("commit_shas")?,
            status: c.parsed("status")?,
            detail: c.opt_text("detail")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for Artifact {
    const TABLE: Table = Table::Artifact;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            opt_id_sql(self.task_id),
            opt_id_sql(self.attempt_id),
            text_sql(&self.kind),
            text_sql(&self.name),
            text_sql(&self.media_type),
            text_sql(&self.path),
            Raw::Int(self.size_bytes),
            text_sql(&self.sha256),
            text_sql(&self.retention),
            opt_ts_sql(self.purged_at),
            object_sql(&self.metadata)?,
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Artifact {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            task_id: c.opt_id("task_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            kind: c.text("kind")?,
            name: c.text("name")?,
            media_type: c.text("media_type")?,
            path: c.text("path")?,
            size_bytes: c.int("size_bytes")?,
            sha256: c.text("sha256")?,
            retention: c.text("retention")?,
            purged_at: c.opt_ts("purged_at")?,
            metadata: c.object("metadata")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for Stack {
    const TABLE: Table = Table::Stack;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.workspace_id),
            id_sql(self.parent_task_id),
            text_sql(self.status.as_str()),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
            opt_text_sql(&self.land_request),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Stack {
            id: c.id("id")?,
            workspace_id: c.id("workspace_id")?,
            parent_task_id: c.id("parent_task_id")?,
            status: c.parsed("status")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
            land_request: c.opt_text("land_request")?,
        })
    }
}

impl TableRow for StackRepository {
    const TABLE: Table = Table::StackRepository;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.stack_id),
            id_sql(self.repository_id),
            text_sql(&self.base_sha),
            text_sql(&self.integration_branch),
            ts_sql(self.created_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(StackRepository {
            stack_id: c.id("stack_id")?,
            repository_id: c.id("repository_id")?,
            base_sha: c.text("base_sha")?,
            integration_branch: c.text("integration_branch")?,
            created_at: c.ts("created_at")?,
        })
    }
}

impl TableRow for StackItem {
    const TABLE: Table = Table::StackItem;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.stack_id),
            id_sql(self.task_id),
            text_sql(self.state.as_str()),
            opt_int_sql(self.merge_order),
            opt_text_sql(&self.detail),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(StackItem {
            id: c.id("id")?,
            stack_id: c.id("stack_id")?,
            task_id: c.id("task_id")?,
            state: c.parsed("state")?,
            merge_order: c.opt_int("merge_order")?,
            detail: c.opt_text("detail")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for Job {
    const TABLE: Table = Table::Job;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            opt_id_sql(self.workspace_id),
            text_sql(&self.kind),
            text_sql(self.status.as_str()),
            Raw::Int(self.priority),
            object_sql(&self.payload)?,
            opt_object_sql(&self.result)?,
            ts_sql(self.run_at),
            Raw::Int(self.attempts),
            Raw::Int(self.max_attempts),
            real_sql(self.backoff_base_seconds),
            opt_real_sql(self.timeout_seconds),
            opt_text_sql(&self.lease_owner),
            opt_ts_sql(self.lease_expires_at),
            opt_ts_sql(self.heartbeat_at),
            opt_ts_sql(self.started_at),
            opt_ts_sql(self.finished_at),
            bool_sql(self.cancel_requested),
            opt_text_sql(&self.idempotency_key),
            strings_sql(&self.resource_keys)?,
            opt_text_sql(&self.last_error),
            opt_text_sql(&self.error_kind),
            opt_id_sql(self.correlation_id),
            opt_id_sql(self.causation_id),
            opt_id_sql(self.task_id),
            opt_id_sql(self.attempt_id),
            text_sql(&self.created_by),
            ts_sql(self.created_at),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Job {
            id: c.id("id")?,
            workspace_id: c.opt_id("workspace_id")?,
            kind: c.text("kind")?,
            status: c.parsed("status")?,
            priority: c.int("priority")?,
            payload: c.object("payload")?,
            result: c.opt_object("result")?,
            run_at: c.ts("run_at")?,
            attempts: c.int("attempts")?,
            max_attempts: c.int("max_attempts")?,
            backoff_base_seconds: c.real("backoff_base_seconds")?,
            timeout_seconds: c.opt_real("timeout_seconds")?,
            lease_owner: c.opt_text("lease_owner")?,
            lease_expires_at: c.opt_ts("lease_expires_at")?,
            heartbeat_at: c.opt_ts("heartbeat_at")?,
            started_at: c.opt_ts("started_at")?,
            finished_at: c.opt_ts("finished_at")?,
            cancel_requested: c.flag("cancel_requested")?,
            idempotency_key: c.opt_text("idempotency_key")?,
            resource_keys: c.strings("resource_keys")?,
            last_error: c.opt_text("last_error")?,
            error_kind: c.opt_text("error_kind")?,
            correlation_id: c.opt_id("correlation_id")?,
            causation_id: c.opt_id("causation_id")?,
            task_id: c.opt_id("task_id")?,
            attempt_id: c.opt_id("attempt_id")?,
            created_by: c.text("created_by")?,
            created_at: c.ts("created_at")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}

impl TableRow for JobRun {
    const TABLE: Table = Table::JobRun;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            id_sql(self.id),
            id_sql(self.job_id),
            Raw::Int(self.attempt_number),
            text_sql(&self.worker_id),
            text_sql(&self.status),
            opt_text_sql(&self.error),
            ts_sql(self.started_at),
            opt_ts_sql(self.finished_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(JobRun {
            id: c.id("id")?,
            job_id: c.id("job_id")?,
            attempt_number: c.int("attempt_number")?,
            worker_id: c.text("worker_id")?,
            status: c.text("status")?,
            error: c.opt_text("error")?,
            started_at: c.ts("started_at")?,
            finished_at: c.opt_ts("finished_at")?,
        })
    }
}

impl TableRow for JobResourceClaim {
    const TABLE: Table = Table::JobResourceClaim;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![id_sql(self.job_id), text_sql(&self.resource_key)])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(JobResourceClaim {
            job_id: c.id("job_id")?,
            resource_key: c.text("resource_key")?,
        })
    }
}

impl TableRow for Worker {
    const TABLE: Table = Table::Worker;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            text_sql(&self.id),
            text_sql(&self.hostname),
            Raw::Int(self.pid),
            text_sql(&self.status),
            Raw::Int(self.concurrency),
            strings_sql(&self.kinds)?,
            strings_sql(&self.current_jobs)?,
            Raw::Int(self.jobs_completed),
            Raw::Int(self.jobs_failed),
            ts_sql(self.started_at),
            ts_sql(self.heartbeat_at),
            opt_ts_sql(self.stopped_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(Worker {
            id: c.text("id")?,
            hostname: c.text("hostname")?,
            pid: c.int("pid")?,
            status: c.text("status")?,
            concurrency: c.int("concurrency")?,
            kinds: c.strings("kinds")?,
            current_jobs: c.strings("current_jobs")?,
            jobs_completed: c.int("jobs_completed")?,
            jobs_failed: c.int("jobs_failed")?,
            started_at: c.ts("started_at")?,
            heartbeat_at: c.ts("heartbeat_at")?,
            stopped_at: c.opt_ts("stopped_at")?,
        })
    }
}

impl TableRow for SystemCursor {
    const TABLE: Table = Table::SystemCursor;

    fn values(&self) -> Result<Vec<Raw>, WgError> {
        Ok(vec![
            text_sql(&self.name),
            Raw::Int(self.value),
            ts_sql(self.updated_at),
        ])
    }

    fn decode(c: &Cols<'_>) -> Result<Self, WgError> {
        Ok(SystemCursor {
            name: c.text("name")?,
            value: c.int("value")?,
            updated_at: c.ts("updated_at")?,
        })
    }
}
